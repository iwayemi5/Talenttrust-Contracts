# Escrow Integration Guide

This guide documents the entrypoints currently implemented by the escrow
contract. Planned features are listed separately and linked to their tracking
issues so integrators can distinguish live API from roadmap.

## Module Map

- `contracts/escrow/src/lib.rs`: contract type, shared API surface, reads, controls, cancellation, reputation, and module wiring.
- `contracts/escrow/src/create_contract.rs`: `create_contract` lifecycle entrypoint.
- `contracts/escrow/src/deposit.rs`: `deposit_funds` lifecycle entrypoint.
- `contracts/escrow/src/release.rs`: `release_milestone` lifecycle entrypoint.
- `contracts/escrow/src/refund.rs`: `refund_unreleased_milestones` lifecycle entrypoint.

## Implemented API Surface

Lifecycle and reputation:

- `create_contract(client, freelancer, milestone_amounts, deposit_mode) -> u32`
- `deposit_funds(contract_id, amount) -> bool`
- `release_milestone(contract_id, milestone_index) -> bool`
- `issue_reputation(contract_id, caller, freelancer, rating) -> bool`
- `cancel_contract(contract_id, caller) -> bool`
- `finalize_contract(contract_id, finalizer) -> bool`

Read-only queries:

- `get_contract(contract_id) -> EscrowContractData`
- `get_finalization_record(contract_id) -> Option<FinalizationRecord>`
- `get_reputation(freelancer) -> Option<ReputationRecord>`
- `get_average_rating(freelancer) -> Option<i128>` — scaled average (see [Average Rating](#average-rating))
- `get_pending_reputation_credits(freelancer) -> u32`
- `get_admin() -> Option<Address>`
- `is_paused() -> bool`
- `is_emergency() -> bool`
- `get_mainnet_readiness_info() -> MainnetReadinessInfo`
- `get_protocol_fee_bps() -> u32`
- `get_accumulated_protocol_fees() -> i128`

Operational controls:

- `initialize(admin) -> bool`
- `pause() -> bool`
- `unpause() -> bool`
- `activate_emergency_pause() -> bool`
- `resolve_emergency() -> bool`

Governance admin transfer (two-step):

- `propose_governance_admin(proposed) -> bool`
- `accept_governance_admin() -> bool`
- `cancel_governance_admin_proposal() -> bool`
- `get_governance_admin() -> Option<Address>`
- `get_pending_governance_admin() -> Option<Address>`

## Canonical Happy Path

### 1. Initialize Operational Admin

```rust
escrow.initialize(&admin);
```

`initialize` is single-use, requires `admin.require_auth()`, and stores the
admin used by pause and emergency controls.

### 2. Create Contract

```rust
let contract_id = escrow.create_contract(
    &client_addr,
    &freelancer_addr,
    &vec![&env, 500_0000000_i128, 500_0000000_i128],
    &DepositMode::ExactTotal,
);
```

Creation requires `client.require_auth()`, rejects identical client/freelancer
addresses, rejects empty or non-positive milestones, caps milestone count at
`MAX_MILESTONES` (rejecting with `TooManyMilestones`), and caps total escrow value
against governed `max_escrow_total_stroops` (rejecting with `TotalCapExceeded`).

### 3. Deposit Funds

```rust
escrow.deposit_funds(&contract_id, &1000_0000000_i128);
```

`ExactTotal` contracts require one exact deposit equal to the milestone total.
`Incremental` contracts allow partial deposits until the milestone total is
reached. Deposits that exceed the required total fail closed.

### 4. Release Milestones

```rust
escrow.release_milestone(&contract_id, &0);
```

Current implementation note: `release_milestone` does not yet authenticate the
client or an arbiter. It validates the contract id, milestone index, unreleased
state, available funded balance, and paused state, then marks the milestone as
released. This authorization gap is intentionally documented here until the auth
fix lands.

When a milestone is released, protocol fees are calculated and accumulated according to the formula:
`fee = (milestone_amount * protocol_fee_bps) / 10_000`
The calculation uses checked arithmetic and panics with `PotentialOverflow` to prevent math errors. Accumulation is routed through `safe_add_amounts`.

When the final milestone is released, status becomes `Completed` and one pending
reputation credit is added for the freelancer.

`PendingReputationCredits` is a non-negative counter that tracks completed
contracts awaiting client-issued reputation for a freelancer. `issue_reputation`
consumes one pending credit and records the rating.

### 5. Issue Reputation

```rust
escrow.issue_reputation(&contract_id, &client_addr, &freelancer_addr, &5_i128);
```

Reputation requires `caller.require_auth()`, the caller must be the stored
client, the freelancer argument must match the contract freelancer, the contract
must be `Completed`, rating must be `1..=5`, and each contract can issue
reputation once.

## Average Rating

`get_average_rating(freelancer) -> Option<i128>` returns the freelancer's
average rating scaled to **basis points (×10 000)**, or `None` if no reputation
record exists or `completed_contracts == 0`.

Formula:

```
result = total_rating * 10_000 / completed_contracts
```

To convert back to the 1–5 decimal scale, divide by `10_000`:

| `total_rating` | `completed_contracts` | `get_average_rating` | Decimal |
|---|---|---|---|
| 5 | 1 | 50 000 | 5.0000 |
| 8 | 2 | 40 000 | 4.0000 |
| 3 | 2 | 15 000 | 1.5000 |

Checked arithmetic is used; overflow and division-by-zero cannot occur.

## Cancellation

```rust
escrow.cancel_contract(&contract_id, &caller);
```

Cancellation requires `caller.require_auth()`. The caller must be the stored
client or freelancer. It is blocked after `Completed` and blocked if the
contract is already `Cancelled`.

## Finalization

```rust
escrow.finalize_contract(&contract_id, &finalizer);
```

Finalization requires `finalizer.require_auth()`. The finalizer must be the
stored client, freelancer, or assigned arbiter. It is allowed only while the
contract status is `Completed` or `Disputed`.

The contract writes one immutable `FinalizationRecord` containing the finalizer,
ledger timestamp, and a `ContractSummary` snapshot. After the record exists,
contract-specific mutating calls reject with `AlreadyFinalized`.

## Two-Step Governance Admin Transfer

The admin transfer uses a propose-accept (two-step) pattern:

```rust
// Step 1: current admin proposes next admin
escrow.propose_governance_admin(&next_admin);

// Step 2: next admin accepts (requires next_admin.require_auth())
escrow.accept_governance_admin();
```

### Rules
- **Self-proposal is rejected**: proposing the current admin as the new admin
  panics with `CannotProposeSelf`.
- **Re-proposing overwrites**: calling `propose_governance_admin` while a
  pending proposal exists silently replaces it (no explicit cancellation
  required).
- **Cancellation**: `cancel_governance_admin_proposal` is admin-gated (only
  the current admin may cancel). It clears the pending admin and emits a
  `("admin", "cancelled")` event.
- **No stale acceptance**: accepting after cancellation panics with
  `InvalidState` because the pending proposal has been removed.
- All operations require the contract to be initialized.

### Events
| Topic | Data | Trigger |
|---|---|---|
| `("admin", "proposed")` | `(admin, proposed, timestamp)` | `propose_governance_admin` |
| `("admin", "accepted")` | `(old_admin, new_admin, timestamp)` | `accept_governance_admin` |
| `("admin", "cancelled")` | `(admin, cancelled_proposal, timestamp)` | `cancel_governance_admin_proposal` |

## Pause and Emergency Controls

`pause`, `unpause`, `activate_emergency_pause`, and `resolve_emergency` require
the stored admin's authorization. While paused or in emergency, mutating
lifecycle calls fail with `ContractPaused`; read-only queries remain available.
`unpause` fails while emergency mode is active.

## Events

Implemented events:

- `("init", "admin_set")` on `initialize`
- `("paused", timestamp)` on `pause`
- `("unpaused", timestamp)` on `unpause`
- `("emergency", "activated")` and `("emergency", "resolved")`
- `("audit", contract_id)` for lifecycle state transitions
- `("created", contract_id)` on contract creation
- `("released", contract_id, milestone_index)` on release
- `("rep_issd", contract_id)` on reputation issuance
- `("cancelled", contract_id)` on cancellation
- `("finalized", contract_id)` on finalization

There is no dedicated deposit event in the current implementation unless the
deposit changes contract status and therefore emits an audit event. Structured
deposit and fee events are planned in
[#336](https://github.com/Talenttrust/Talenttrust-Contracts/issues/336).

## Implemented Security Assumptions

- Creation and reputation issue require explicit address authentication.
- Pause and emergency controls are admin-authenticated.
- Admin transfer uses a two-step propose-accept pattern. The current admin
  cannot propose themselves. Only the current admin can cancel a pending
  proposal. Re-proposing overwrites without error.
- Deposits cannot exceed the exact milestone total.
- Releases fail on duplicate milestone release, invalid milestone id, missing
  contract, paused state, and insufficient funded balance.
- Arithmetic for escrow totals, deposits, and releases uses checked helpers and
  panics with `PotentialOverflow` on overflow.
- Accounting is checked after balance-changing operations.
- The contract stores accounting state only; token custody and token transfers
  are not implemented in `lib.rs` and must be handled by an audited integration.
- Storage uses persistent keys. TTL constants exist for planned pending approval
  and migration flows, but no current public entrypoint writes those pending
  records.

## Planned Features

These features are not implemented entrypoints today:

- Protocol fee deduction on release: planned in
  [#313](https://github.com/Talenttrust/Talenttrust-Contracts/issues/313).
- Protocol fee treasury withdrawal: planned in
  [#314](https://github.com/Talenttrust/Talenttrust-Contracts/issues/314).
- Governed parameter setter/readiness wiring: planned in
  [#323](https://github.com/Talenttrust/Talenttrust-Contracts/issues/323).
- Structured deposit and fee events: planned in
  [#336](https://github.com/Talenttrust/Talenttrust-Contracts/issues/336).
- Storage-key reference for declared-but-unused keys, including pending client
  migration and protocol fee keys: planned in
  [#342](https://github.com/Talenttrust/Talenttrust-Contracts/issues/342).
- `migrate_state` / `StateV1` / `StateV2` migration flow: not implemented;
  tracked by this reconciliation issue
  [#341](https://github.com/Talenttrust/Talenttrust-Contracts/issues/341)
  until a dedicated implementation issue exists.

Any documentation that describes one of these items as available should be
treated as roadmap text, not live integration guidance.


### Milestone Approval and Revocation

Participants can approve milestone items prior to fund distribution payouts. If an authorization mistake is discovered prior to complete disbursement release configurations, the approving party can rescind authority.

#### `revoke_approval(contract_id: Address, caller: Address, milestone_index: u32)`
- **Authorization Required:** `caller.require_auth()`
- **Behavior:** Explicitly removes individual state flags (`client_approved` | `freelancer_approved` | `arbiter_approved`). When all structural components drop to `false`, temporary records are scrubbed entirely to maximize gas savings.
- **Errors raised:** `Error::MilestoneAlreadyReleased`, `Error::ApprovalRecordNotFound`.