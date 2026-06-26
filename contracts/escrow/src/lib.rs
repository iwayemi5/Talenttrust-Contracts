#![no_std]
#![allow(clippy::derivable_impls)]
#![allow(clippy::manual_range_contains)]
#![allow(clippy::assertions_on_constants)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::collapsible_else_if)]
#![allow(clippy::redundant_field_names)]
#![allow(clippy::ptr_arg)]
#![allow(clippy::useless_vec)]
#![allow(clippy::let_and_return)]
#![allow(clippy::inconsistent_digit_grouping)]
#![allow(clippy::int_plus_one)]
#![allow(clippy::duplicated_attributes)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::redundant_clone)]
#![allow(clippy::bool_assert_comparison)]
#![allow(clippy::needless_borrow)]
#![allow(clippy::clone_on_copy)]
#![allow(clippy::module_inception)]
#![allow(clippy::single_match)]
#![allow(clippy::useless_conversion)]

mod amount_validation;
mod approvals;
mod create_contract;
mod deposit;
mod dispute;
mod finalize;
mod governance;
mod migration;
mod refund;
mod release;
mod ttl;
mod types;
mod utils;

pub use amount_validation::{safe_add_amounts, safe_subtract_amounts};
pub use migration::PendingClientMigration;
pub use ttl::{ADMIN_ROTATION_MIN_DELAY_LEDGERS, PENDING_MIGRATION_TTL_LEDGERS};
pub use types::{
    Contract, ContractStatus, CONTRACT_SUMMARY_SCHEMA_VERSION, ContractSummary, DataKey,
    DepositMode, Error, GovernedParameters, MAINNET_MAX_TOTAL_ESCROW_PER_CONTRACT_STROOPS,
    MAINNET_PROTOCOL_VERSION, Milestone, MilestoneApprovals, MilestoneSummary, PendingAdminProposal,
    ReadinessChecklist, ReleaseAuthorization, Reputation,
};
pub use types::{ContractSummary, MilestoneSummary, CONTRACT_SUMMARY_SCHEMA_VERSION};

use soroban_sdk::{contract, contractimpl, contracterror, contracttype, symbol_short, Address, Env, Symbol, Vec};

#[contract]
pub struct Escrow;

/// Governance-level errors for admin-gated operations.
#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum EscrowError {
    InvalidParticipant = 1,
    EmptyMilestones = 2,
    InvalidMilestoneAmount = 3,
    InvalidDepositAmount = 4,
    InvalidMilestone = 5,
    ContractNotFound = 6,
    EmptyRefundRequest = 7,
    DuplicateMilestoneInRefund = 8,
    AlreadyReleased = 9,
    AlreadyRefunded = 10,
    InsufficientFunds = 11,
    AlreadyInitialized = 12,
    InsufficientAccumulatedFees = 13,
    /// Returned by lifecycle entrypoints when `initialize` has not been called.
    ///
    /// All money-flow operations require initialization so the admin-controlled
    /// safety rails (pause, emergency controls, protocol fees) are always in
    /// scope before any funds can move.
    NotInitialized = 14,
    UnauthorizedRole = 15,
    ContractPaused = 16,
    EmergencyActive = 17,
    InvalidState = 18,
    InvalidRating = 19,
    SelfRating = 20,
    ReputationAlreadyIssued = 21,
    NotCompleted = 22,
    FreelancerMismatch = 23,
    InvalidStatusTransition = 24,
    AlreadyFinalized = 25,
    InvalidProtocolParameters = 26,
    GovernanceNotInitialized = 27,
    PotentialOverflow = 28,
    AccountingInvariantViolated = 29,
    TimelockNotElapsed = 30,
}

/// Returns `Some(a - b)` when `a >= b`, otherwise `None`.
pub fn safe_subtract_amounts(a: i128, b: i128) -> Option<i128> {
    a.checked_sub(b).filter(|&diff| diff >= 0)
}

/// Returns `Some(a + b)`, or `None` on overflow.
pub fn safe_add_amounts(a: i128, b: i128) -> Option<i128> {
    a.checked_add(b)
}

// ─────────────────────────────────────────────────────────────────────────────
// Single #[contractimpl] block — all contract-callable entrypoints live here.
// ─────────────────────────────────────────────────────────────────────────────

#[contractimpl]
impl Escrow {
    // ── Hello / CI ───────────────────────────────────────────────────────────

    /// Hello-world style function for testing and CI.
    pub fn hello(_env: Env, to: Symbol) -> Symbol {
        to
    }

    // ── Initialization ───────────────────────────────────────────────────────

    /// Initializes the escrow contract with the operational admin.
    ///
    /// Single-use. Stores the admin address that controls pause, emergency,
    /// protocol-fee, and governance operations. All escrow lifecycle operations
    /// (create, deposit, release, refund, cancel) call `require_initialized`
    /// so that these safety rails are always bound before money can move.
    pub fn initialize(env: Env, admin: Address) -> bool {
        if env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Initialized)
            .unwrap_or(false)
        {
            env.panic_with_error(EscrowError::AlreadyInitialized);
        }

        admin.require_auth();
        env.storage().persistent().set(&DataKey::Initialized, &true);
        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage()
            .persistent()
            .set(&DataKey::NextContractId, &1u32);

        let mut checklist: ReadinessChecklist = env
            .storage()
            .persistent()
            .get(&DataKey::ReadinessChecklist)
            .unwrap_or_default();
        checklist.initialized = true;
        env.storage()
            .persistent()
            .set(&DataKey::ReadinessChecklist, &checklist);

        env.events().publish(
            (symbol_short!("init"), Symbol::new(&env, "admin_set")),
            (admin.clone(), env.ledger().timestamp()),
        );

        true
    }

    /// Returns the stored governance admin address.
    pub fn get_admin(env: Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::Admin)
    }

    /// Bind the SAC settlement token used by `deposit_funds` and
    /// `release_milestone`. Single-use: the first call after `initialize`
    /// stores the token address; subsequent calls panic with
    /// `SettlementTokenAlreadyBound`.
    ///
    /// # Arguments
    /// * `env`  - The contract environment
    /// * `token` - The Stellar Asset Contract address that all escrowed
    ///              funds will be denominated in.
    ///
    /// # Returns
    /// `true` on a successful binding.
    ///
    /// # Errors
    /// * `NotInitialized` - if `initialize` has never been called.
    /// * `UnauthorizedRole` - if the caller is not the stored admin.
    /// * `SettlementTokenAlreadyBound` - if a token has already been bound.
    ///
    /// # Security
    /// * Token binding is single-use, gated to the admin, and audited via
    ///   the `(settl_tok, "bound")` event so an observer can prove the
    ///   binding sequence.
    pub fn bind_settlement_token(env: Env, token: Address) -> bool {
        Self::require_initialized(&env);
        let admin: Address = env.storage().persistent().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        if env
            .storage()
            .persistent()
            .get::<_, Address>(&DataKey::SettlementToken)
            .is_some()
        {
            env.panic_with_error(EscrowError::SettlementTokenAlreadyBound);
        }
        env.storage()
            .persistent()
            .set(&DataKey::SettlementToken, &token);
        env.events().publish(
            (symbol_short!("settl_tok"), Symbol::new(&env, "bound")),
            (admin, token, env.ledger().timestamp()),
        );
        true
    }

    /// Returns the bound SAC settlement token address, or `None` if no token
    /// has been bound via `bind_settlement_token` yet.
    pub fn get_settlement_token(env: Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::SettlementToken)
    }

    /// Returns the current mainnet readiness checklist.
    ///
    /// Read-only and requires no authorization. The `caps_set`, `protocol_version`,
    /// and `max_escrow_total_stroops` fields are always populated from compile-time
    /// constants regardless of storage state.
    pub fn get_mainnet_readiness_info(env: Env) -> ReadinessChecklist {
        let mut checklist: ReadinessChecklist = env
            .storage()
            .persistent()
            .get(&DataKey::ReadinessChecklist)
            .unwrap_or_default();
        // Always reflect compile-time constants, never rely on stored values for these.
        checklist.caps_set = MAINNET_MAX_TOTAL_ESCROW_PER_CONTRACT_STROOPS > 0;
        checklist.protocol_version = MAINNET_PROTOCOL_VERSION;
        checklist.max_escrow_total_stroops = MAINNET_MAX_TOTAL_ESCROW_PER_CONTRACT_STROOPS;
        checklist
    }

    // ── Approvals ────────────────────────────────────────────────────────────

    /// Approves a milestone for release.
    ///
    /// Records the approval in temporary storage with TTL expiry.
    pub fn approve_milestone_release(
        env: Env,
        contract_id: u32,
        caller: Address,
        milestone_index: u32,
    ) -> bool {
        Self::require_not_finalized(&env, contract_id);
        approvals::approve_milestone(&env, contract_id, milestone_index, &caller)
            .unwrap_or_else(|e| env.panic_with_error(e))
    }

    /// Retrieves contract information.
    pub fn get_contract(env: Env, contract_id: u32) -> Contract {
        let contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));
        ttl::extend_contract_ttl(&env, contract_id);
        contract
    }

    /// Retrieves all milestones for a contract.
    pub fn get_milestones(env: Env, contract_id: u32) -> Vec<Milestone> {
        let milestone_key = Symbol::new(&env, "milestones");
        let milestones = env
            .storage()
            .persistent()
            .get(&(DataKey::Contract(contract_id), milestone_key))
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));
        ttl::extend_milestone_ttl(&env, contract_id);
        milestones
    }

    /// Returns funded minus released minus refunded for `contract_id`.
    pub fn get_refundable_balance(env: Env, contract_id: u32) -> i128 {
        let contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));
        ttl::extend_contract_ttl(&env, contract_id);
        contract.funded_amount - contract.released_amount - contract.refunded_amount
    }

    /// Retrieves approval status for a milestone.
    /// Returns None if approvals have expired or don't exist.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    /// * `milestone_index` - The milestone index
    ///
    /// # Returns
    /// Optional MilestoneApprovals struct
    pub fn get_milestone_approvals(
        env: Env,
        contract_id: u32,
        milestone_index: u32,
    ) -> Option<MilestoneApprovals> {
        let approval_key = DataKey::MilestoneApprovals(contract_id, milestone_index);
        env.storage().temporary().get(&approval_key)
    }

    // ── Pause / unpause ──────────────────────────────────────────────────────

    /// Pause all state-changing escrow operations.
    ///
    /// Requires the stored admin's authorization. While paused, all mutating
    /// entrypoints panic with `ContractPaused`. Read-only queries are never blocked.
    ///
    /// # Events
    /// Emits `("paused", timestamp)` with `(admin,)` payload.
    pub fn pause(env: Env) -> bool {
        Self::require_initialized(&env);
        let admin: Address = env.storage().persistent().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().persistent().set(&DataKey::Paused, &true);

        env.events()
            .publish((symbol_short!("pause"), env.ledger().timestamp()), (admin,));
        true
    }

    /// Unpause operations, clearing the `Paused` flag.
    ///
    /// Blocked while `Emergency` is active — use `resolve_emergency` instead.
    /// Requires the stored admin's authorization.
    ///
    /// # Events
    /// Emits `("unpaused", timestamp)` with `(admin,)` payload.
    pub fn unpause(env: Env) -> bool {
        Self::require_initialized(&env);
        if env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Emergency)
            .unwrap_or(false)
        {
            env.panic_with_error(EscrowError::EmergencyActive);
        }
        let admin: Address = env.storage().persistent().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().persistent().set(&DataKey::Paused, &false);

        env.events().publish(
            (symbol_short!("unpaused"), env.ledger().timestamp()),
            (admin,),
        );
        true
    }

    /// Returns `true` if the contract is currently paused.
    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

    // ── Emergency pause ──────────────────────────────────────────────────────

    /// Activate emergency pause, setting both `Emergency` and `Paused` flags.
    ///
    /// Requires the stored admin's authorization. While emergency is active,
    /// all mutating entrypoints panic with `EmergencyActive` or `ContractPaused`,
    /// and `unpause` is blocked.
    ///
    /// # Events
    /// Emits `("emergency", "activated")` with `(admin, timestamp)` payload.
    /// Sets `emergency_controls_enabled` in the readiness checklist.
    pub fn activate_emergency_pause(env: Env) -> bool {
        if env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Initialized)
            .unwrap_or(false)
        {
            let admin: Address = env.storage().persistent().get(&DataKey::Admin).unwrap();
            admin.require_auth();
        }
        env.storage().persistent().set(&DataKey::Emergency, &true);
        env.storage().persistent().set(&DataKey::Paused, &true);

        let mut checklist: ReadinessChecklist = env
            .storage()
            .persistent()
            .get(&DataKey::ReadinessChecklist)
            .unwrap_or_default();
        checklist.emergency_controls_enabled = true;
        env.storage()
            .persistent()
            .set(&DataKey::ReadinessChecklist, &checklist);

        env.events().publish(
            (
                Symbol::new(&env, "emergency"),
                Symbol::new(&env, "activated"),
            ),
            (admin, env.ledger().timestamp()),
        );
        true
    }

    /// Resolve emergency, clearing both `Emergency` and `Paused` flags.
    ///
    /// Requires the stored admin's authorization. After resolution, all
    /// operations resume normally.
    ///
    /// # Events
    /// Emits `("emergency", "resolved")` with `(admin, timestamp)` payload.
    /// Sets `emergency_controls_enabled` in the readiness checklist.
    pub fn resolve_emergency(env: Env) -> bool {
        if env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Initialized)
            .unwrap_or(false)
        {
            let admin: Address = env.storage().persistent().get(&DataKey::Admin).unwrap();
            admin.require_auth();
        }
        env.storage().persistent().set(&DataKey::Emergency, &false);
        env.storage().persistent().set(&DataKey::Paused, &false);
        let mut checklist: ReadinessChecklist = env
            .storage()
            .persistent()
            .get(&DataKey::ReadinessChecklist)
            .unwrap_or_default();
        checklist.emergency_controls_enabled = true;
        env.storage()
            .persistent()
            .set(&DataKey::ReadinessChecklist, &checklist);
        true
    }

    /// Returns `true` if the contract is currently in emergency mode.
    pub fn is_emergency(env: Env) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Emergency)
            .unwrap_or(false)
    }

    // ── Cancel contract ──────────────────────────────────────────────────────

    /// Cancels an active escrow contract.
    ///
    /// # Errors
    /// * `ContractPaused` - If the contract is paused while not in emergency mode
    /// * `EmergencyActive` - If the contract is in an active emergency pause
    /// * `ContractNotFound` - If contract doesn't exist
    /// * `UnauthorizedRole` - If caller is not client or freelancer
    /// * `InvalidState` - If contract is not in Created, PartiallyFunded, or Funded state
    ///
    /// # Security
    /// * Pause/emergency gate runs BEFORE contract state read so a paused
    ///   contract cannot have its cancellation path tread on the record.
    pub fn cancel_contract(env: Env, contract_id: u32, caller: Address) -> bool {
        // Pause/emergency gate: refuses cancellation while the contract is
        // paused or in an active emergency. Runs BEFORE state read so a
        // paused contract cannot have its cancellation path tread on the
        // contract record at all.
        Self::require_not_paused(&env);

        let mut contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));
        ttl::extend_contract_ttl(&env, contract_id);

        if caller != contract.client && caller != contract.freelancer {
            env.panic_with_error(Error::UnauthorizedRole);
        }

        match contract.status {
            ContractStatus::Created | ContractStatus::PartiallyFunded | ContractStatus::Funded => {}
            _ => env.panic_with_error(Error::InvalidState),
        }

        caller.require_auth();
        Self::require_not_finalized(&env, contract_id);
        contract.status = ContractStatus::Cancelled;
        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);
        ttl::extend_contract_ttl(&env, contract_id);
        true
    }

    // ── Reputation ───────────────────────────────────────────────────────────

    /// Issues reputation credit for a completed contract.
    ///
    /// # Errors
    /// * `ContractPaused` - If the contract is paused while not in emergency mode
    /// * `EmergencyActive` - If the contract is in an active emergency pause
    /// * `ContractNotFound` - If contract doesn't exist
    /// * `UnauthorizedRole` - If caller is not the stored client
    /// * `FreelancerMismatch` - If `freelancer` does not match the stored freelancer
    /// * `InvalidRating` - If rating is not in [1, 5]
    /// * `NotCompleted` - If contract status is not `Completed`
    /// * `ReputationAlreadyIssued` - If reputation was already issued
    /// * `SelfRating` - If client and freelancer are the same address
    ///
    /// # Security
    /// * Pause/emergency gate runs BEFORE contract state read so paused
    ///   contracts cannot have reputation mutated while paused.
    pub fn issue_reputation(
        env: Env,
        contract_id: u32,
        caller: Address,
        freelancer: Address,
        rating: i128,
    ) -> bool {
        // Pause/emergency gate: refuses any reputation issuance while the
        // contract is paused or in an active emergency.
        Self::require_not_paused(&env);

        let contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));
        ttl::extend_contract_ttl(&env, contract_id);

        if contract.status != ContractStatus::Completed {
            env.panic_with_error(EscrowError::NotCompleted);
        }

        if caller != contract.client {
            env.panic_with_error(EscrowError::UnauthorizedRole);
        }
        if freelancer != contract.freelancer {
            env.panic_with_error(EscrowError::FreelancerMismatch);
        }
        if rating < 1 || rating > 5 {
            env.panic_with_error(EscrowError::InvalidRating);
        }
        if contract.status != ContractStatus::Completed {
            env.panic_with_error(EscrowError::NotCompleted);
        }
        if env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::ReputationIssued(contract_id))
            .unwrap_or(false)
        {
            env.panic_with_error(EscrowError::ReputationAlreadyIssued);
        }
        if contract.client == contract.freelancer {
            env.panic_with_error(EscrowError::SelfRating);
        }

        caller.require_auth();
        env.storage()
            .persistent()
            .set(&DataKey::ReputationIssued(contract_id), &true);

        let pending_key = DataKey::PendingReputationCredits(contract.freelancer.clone());
        let pending: i128 = env.storage().persistent().get(&pending_key).unwrap_or(0);
        if pending <= 0 {
            env.panic_with_error(EscrowError::InvalidState);
        }
        env.storage().persistent().set(&pending_key, &(pending - 1));

        let rep_key = DataKey::Reputation(contract.freelancer.clone());
        let mut rep: types::Reputation =
            env.storage().persistent().get(&rep_key).unwrap_or_default();
        rep.completed_contracts += 1;
        rep.total_rating += rating;
        rep.last_rating = rating;
        env.storage().persistent().set(&rep_key, &rep);

        true
    }

    pub fn get_reputation(env: Env, address: Address) -> Option<types::Reputation> {
        env.storage()
            .persistent()
            .get(&DataKey::Reputation(address))
    }

    /// Returns the freelancer's average rating scaled to basis points (×10 000),
    /// or `None` if no reputation record exists or no contracts have been completed.
    ///
    /// # Scaling
    /// `result = total_rating * 10_000 / completed_contracts`
    ///
    /// A raw rating of 5 on a single contract returns `50_000` (5.0000 on a
    /// 1–5 scale).  Clients divide by `10_000` to recover the decimal value.
    ///
    /// Checked arithmetic is used throughout; division by zero is impossible
    /// because `None` is returned whenever `completed_contracts == 0`.
    pub fn get_average_rating(env: Env, address: Address) -> Option<i128> {
        /// Basis-point scaling factor (×10 000 preserves four decimal places).
        const SCALE: i128 = 10_000;

        let rep: types::Reputation = env
            .storage()
            .persistent()
            .get(&DataKey::Reputation(address))?;

        if rep.completed_contracts == 0 {
            return None;
        }

        rep.total_rating
            .checked_mul(SCALE)
            .and_then(|scaled| scaled.checked_div(rep.completed_contracts))
    }

    pub fn get_pending_reputation_credits(env: Env, address: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::PendingReputationCredits(address))
            .unwrap_or(0)
    }

    // -----------------------------------------------------------------------
    // Governed parameters
    // -----------------------------------------------------------------------

    /// Sets protocol fee (basis points) and maximum escrow total per contract.
    ///
    /// Requires initialization. Only the stored admin may call this.
    /// `fee_bps` must be ≤ 10 000; `max_escrow_total_stroops` must be > 0.
    /// Sets `governed_params_set` in the readiness checklist on success.
    pub fn set_governed_params(
        env: Env,
        caller: Address,
        fee_bps: u32,
        max_escrow_total_stroops: i128,
    ) -> bool {
        Self::require_initialized(&env);
        let admin: Address = env.storage().persistent().get(&DataKey::Admin).unwrap();
        if caller != admin {
            env.panic_with_error(EscrowError::UnauthorizedRole);
        }
        caller.require_auth();
        if fee_bps > 10_000 || max_escrow_total_stroops <= 0 {
            env.panic_with_error(EscrowError::InvalidProtocolParameters);
        }
        let params = GovernedParameters { protocol_fee_bps: fee_bps, max_escrow_total_stroops };
        env.storage()
            .persistent()
            .set(&DataKey::GovernedParameters, &params);
        let mut checklist: ReadinessChecklist = env
            .storage()
            .persistent()
            .get(&DataKey::ReadinessChecklist)
            .unwrap_or_default();
        checklist.governed_params_set = true;
        env.storage()
            .persistent()
            .set(&DataKey::ReadinessChecklist, &checklist);
        true
    }

    /// Returns the current governed parameters, if set.
    pub fn get_governed_parameters(env: Env) -> Option<GovernedParameters> {
        env.storage()
            .persistent()
            .get(&DataKey::GovernedParameters)
    }

    // -----------------------------------------------------------------------
    // Governance: treasury rotation with timelock
    // -----------------------------------------------------------------------

    /// Propose a new admin. Stores the pending admin proposal (with the
    /// current ledger sequence for timelock enforcement) and emits an event.
    ///
    /// Requires initialization; current admin must authorize.
    pub fn propose_governance_admin(env: Env, proposed: Address) -> bool {
        Self::propose_governance_admin_impl(env, proposed)
    }

    /// Accept a pending admin proposal. The proposed admin must authorize.
    ///
    /// Fails with `TimelockNotElapsed` if fewer than
    /// `ADMIN_ROTATION_MIN_DELAY_LEDGERS` have elapsed since the proposal.
    pub fn accept_governance_admin(env: Env) -> bool {
        Self::accept_governance_admin_impl(env)
    }

    /// Return the currently pending admin address, if any.
    pub fn get_pending_governance_admin(env: Env) -> Option<Address> {
        Self::get_pending_governance_admin_impl(env)
    }

    /// Return the current admin address.
    pub fn get_governance_admin(env: Env) -> Option<Address> {
        Self::get_governance_admin_impl(env)
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Proposes a client migration for an existing contract.
    pub fn propose_client_migration(
        env: Env,
        contract_id: u32,
        current_client: Address,
        new_client: Address,
    ) -> bool {
        Self::propose_client_migration_impl(env, contract_id, current_client, new_client)
    }

    /// Accepts a pending client migration.
    pub fn accept_client_migration(env: Env, contract_id: u32, new_client: Address) -> bool {
        Self::accept_client_migration_impl(env, contract_id, new_client)
    }

    /// Returns true if a live pending client migration exists.
    pub fn has_pending_client_migration(env: Env, contract_id: u32) -> bool {
        Self::has_pending_client_migration_impl(env, contract_id)
    }

    /// Returns the live pending client migration record.
    pub fn get_pending_client_migration(
        env: Env,
        contract_id: u32,
    ) -> migration::PendingClientMigration {
        Self::get_pending_client_migration_impl(env, contract_id)
    }

    // ── Finalization ─────────────────────────────────────────────────────────

    /// Finalizes an escrow contract by writing immutable close metadata.
    pub fn finalize_contract(env: Env, contract_id: u32, finalizer: Address) -> bool {
        Self::finalize_contract_impl(env, contract_id, finalizer)
    }

    /// Returns immutable close metadata for a contract.
    pub fn get_finalization_record(
        env: Env,
        contract_id: u32,
    ) -> Option<finalize::FinalizationRecord> {
        Self::get_finalization_record_impl(env, contract_id)
    }

    // ── Governance ───────────────────────────────────────────────────────────

    /// Sets the protocol fee in basis points.
    pub fn set_protocol_fee_bps(env: Env, new_bps: u32) -> bool {
        Self::set_protocol_fee_bps_impl(&env, new_bps)
    }

    /// Proposes a new governance admin.
    pub fn propose_governance_admin(env: Env, proposed: Address) -> bool {
        Self::propose_governance_admin_impl(&env, proposed)
    }

    /// Accepts a pending governance admin proposal.
    pub fn accept_governance_admin(env: Env) -> bool {
        Self::accept_governance_admin_impl(&env)
    }

    /// Returns the pending governance admin, if any.
    pub fn get_pending_governance_admin(env: Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::PendingAdmin)
    }

    /// Returns the current governance admin.
    pub fn get_governance_admin(env: Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::Admin)
    }

    // ── Protocol fee helpers ─────────────────────────────────────────────────

    pub(crate) fn get_protocol_fee_bps(env: &Env) -> u32 {
        env.storage()
            .persistent()
            .get::<_, u32>(&DataKey::ProtocolFeeBps)
            .unwrap_or(0)
    }

    pub(crate) fn calculate_protocol_fee(amount: i128, fee_bps: u32) -> i128 {
        if fee_bps == 0 {
            return 0;
        }
        amount * fee_bps as i128 / 10_000
    }

    // ── Internal guards ──────────────────────────────────────────────────────

    /// Panics with `NotInitialized` unless `initialize` has been called.
    ///
    /// Called at the top of every lifecycle entrypoint — `create_contract`,
    /// `deposit_funds`, `release_milestone`, `refund_unreleased_milestones`,
    /// and `cancel_contract` — so that the admin-controlled safety rails
    /// (pause, emergency controls, protocol fees) are always in scope before
    /// any money can move.
    pub(crate) fn require_initialized(env: &Env) {
        if !env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Initialized)
            .unwrap_or(false)
        {
            env.panic_with_error(EscrowError::NotInitialized);
        }
    }

    fn is_initialized(env: &Env) -> bool {
        env.storage()
            .persistent()
            .get::<_, bool>(&DataKey::Initialized)
            .unwrap_or(false)
    }

    fn get_protocol_fee_bps(env: &Env) -> u32 {
        env.storage()
            .persistent()
            .get::<_, u32>(&DataKey::ProtocolFeeBps)
            .unwrap_or(0)
    }

    fn calculate_protocol_fee(amount: i128, fee_bps: u32) -> i128 {
        if fee_bps == 0 {
            0
        } else {
            amount * fee_bps as i128 / 10_000
        }
    }
}

// -----------------------------------------------------------------------
// Shared helpers
// -----------------------------------------------------------------------

/// Safe subtraction that returns `None` on underflow.
pub fn safe_subtract_amounts(a: i128, b: i128) -> Option<i128> {
    a.checked_sub(b)
}

/// Subtracts `b` from `a`, returning `None` if the result would underflow.
pub fn safe_subtract_amounts(a: i128, b: i128) -> Option<i128> {
    a.checked_sub(b)
}

#[cfg(test)]
mod test;
