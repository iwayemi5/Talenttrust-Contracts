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

pub use amount_validation::safe_subtract_amounts;
pub use migration::PendingClientMigration;
pub use ttl::PENDING_MIGRATION_TTL_LEDGERS;
pub use types::{
    Contract, ContractStatus, ContractSummary, DataKey, Error, Milestone, MilestoneApprovals,
    MilestoneSummary, ReadinessChecklist, ReleaseAuthorization, Reputation,
    CONTRACT_SUMMARY_SCHEMA_VERSION,
};
pub use types::{ContractSummary, MilestoneSummary, CONTRACT_SUMMARY_SCHEMA_VERSION};

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, Symbol, Vec,
};

#[contract]
pub struct Escrow;

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
    PotentialOverflow = 25,
    AccountingInvariantViolated = 26,
    AlreadyFinalized = 27,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractData {
    pub client: Address,
    pub freelancer: Address,
    pub milestones: Vec<i128>,
}

#[contractimpl]
impl Escrow {
    /// Hello-world style function for testing and CI.
    pub fn hello(_env: Env, to: Symbol) -> Symbol {
        to
    }

    /// Initializes the escrow contract with the operational admin.
    ///
    /// This call is single-use and stores the admin address for future
    /// admin-gated entrypoints such as `withdraw_protocol_fees`.
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

    /// Returns the stored governance admin address, if one has been initialized.
    pub fn get_admin(env: Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::Admin)
    }

    /// Returns the current mainnet readiness checklist.
    pub fn get_mainnet_readiness_info(env: Env) -> ReadinessChecklist {
        env.storage()
            .persistent()
            .get(&DataKey::ReadinessChecklist)
            .unwrap_or_default()
    }

    /// Approves a milestone for release.
    ///
    /// Records the approval in temporary storage with TTL expiry.
    /// Approvals automatically expire after PENDING_APPROVAL_TTL_LEDGERS.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    /// * `caller` - The address of the caller (must be authorized)
    /// * `milestone_index` - The index of the milestone to approve
    ///
    /// # Returns
    /// `true` if approval was recorded successfully
    ///
    /// # Errors
    /// * `ContractPaused` - If contract is paused or in emergency
    /// * `ContractNotFound` - If contract doesn't exist
    /// * `InvalidState` - If contract is not in Funded state
    /// * `IndexOutOfBounds` - If milestone index is invalid
    /// * `MilestoneAlreadyReleased` - If milestone was already released
    /// * `UnauthorizedRole` - If caller is not authorized to approve
    /// * `AlreadyApproved` - If caller has already approved this milestone
    ///
    /// # Security
    /// - Caller must be authenticated
    /// - Only authorized parties can approve based on ReleaseAuthorization mode
    /// - Approvals expire via TTL and are auto-evicted
    /// - Duplicate approvals are rejected
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
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    ///
    /// # Returns
    /// The contract data
    ///
    /// # Errors
    /// * `ContractNotFound` - If contract doesn't exist
    pub fn get_contract(env: Env, contract_id: u32) -> Contract {
        let contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));

        // Extend TTL on contract read
        ttl::extend_contract_ttl(&env, contract_id);

        contract
    }

    /// Retrieves all milestones for a contract.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    ///
    /// # Returns
    /// Vector of milestones
    ///
    /// # Errors
    /// * `ContractNotFound` - If contract doesn't exist
    pub fn get_milestones(env: Env, contract_id: u32) -> Vec<Milestone> {
        ttl::extend_contract_ttl(&env, contract_id);
        ttl::load_milestones(&env, contract_id)
    }

    /// Calculates the refundable balance (funded but not released or refunded).
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    ///
    /// # Returns
    /// The refundable balance amount
    ///
    /// # Errors
    /// * `ContractNotFound` - If contract doesn't exist
    pub fn get_refundable_balance(env: Env, contract_id: u32) -> i128 {
        let contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));

        // Extend TTL on contract read
        ttl::extend_contract_ttl(&env, contract_id);

        contract.funded_amount - contract.released_amount - contract.refunded_amount
    }

    /// Retrieves approval status for a milestone.
    ///
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

    /// Revokes the caller's active approval for a milestone.
    ///
    /// The approval record is removed when no approval flags remain set.
    pub fn revoke_approval(
        env: Env,
        contract_id: u32,
        caller: Address,
        milestone_index: u32,
    ) -> bool {
        caller.require_auth();

        let contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));

        ttl::extend_contract_ttl(&env, contract_id);
        Self::require_not_finalized(&env, contract_id);

        let milestone_key = Symbol::new(&env, "milestones");
        let milestones: Vec<Milestone> = env
            .storage()
            .persistent()
            .get(&(DataKey::Contract(contract_id), milestone_key))
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));

        ttl::extend_milestone_ttl(&env, contract_id);

        if milestone_index >= milestones.len() {
            env.panic_with_error(Error::IndexOutOfBounds);
        }

        let milestone = milestones.get(milestone_index).unwrap();
        if milestone.released {
            env.panic_with_error(Error::MilestoneAlreadyReleased);
        }
        if milestone.refunded {
            env.panic_with_error(Error::AlreadyRefunded);
        }

        let approval_key = DataKey::MilestoneApprovals(contract_id, milestone_index);
        let mut approvals: MilestoneApprovals = env
            .storage()
            .temporary()
            .get(&approval_key)
            .unwrap_or_else(|| env.panic_with_error(Error::InsufficientApprovals));

        let mut revoked = false;
        if caller == contract.client {
            if approvals.client_approved {
                approvals.client_approved = false;
                revoked = true;
            }
        } else if caller == contract.freelancer {
            if approvals.freelancer_approved {
                approvals.freelancer_approved = false;
                revoked = true;
            }
        } else if contract.arbiter.as_ref() == Some(&caller) {
            if approvals.arbiter_approved {
                approvals.arbiter_approved = false;
                revoked = true;
            }
        }

        if !revoked {
            env.panic_with_error(Error::UnauthorizedRole);
        }

        if !approvals.client_approved
            && !approvals.freelancer_approved
            && !approvals.arbiter_approved
        {
            env.storage().temporary().remove(&approval_key);
        } else {
            env.storage().temporary().set(&approval_key, &approvals);
            env.storage().temporary().extend_ttl(
                &approval_key,
                ttl::PENDING_APPROVAL_BUMP_THRESHOLD,
                ttl::PENDING_APPROVAL_TTL_LEDGERS,
            );
        }

        true
    }

    // -----------------------------------------------------------------------
    // Pause / unpause
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // Emergency pause
    // -----------------------------------------------------------------------

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
        Self::require_initialized(&env);
        let admin: Address = env.storage().persistent().get(&DataKey::Admin).unwrap();
        admin.require_auth();
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
        Self::require_initialized(&env);
        let admin: Address = env.storage().persistent().get(&DataKey::Admin).unwrap();
        admin.require_auth();
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

        env.events().publish(
            (
                Symbol::new(&env, "emergency"),
                Symbol::new(&env, "resolved"),
            ),
            (admin, env.ledger().timestamp()),
        );
        true
    }

    /// Returns `true` if the contract is currently in emergency mode.
    pub fn is_emergency(env: Env) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Emergency)
            .unwrap_or(false)
    }

    // -----------------------------------------------------------------------
    // Cancel contract
    // -----------------------------------------------------------------------

    pub fn cancel_contract(env: Env, contract_id: u32, caller: Address) -> bool {
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
        contract.status = ContractStatus::Cancelled;
        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);
        ttl::extend_contract_ttl(&env, contract_id);
        true
    }

    // -----------------------------------------------------------------------
    // Reputation
    // -----------------------------------------------------------------------

    pub fn issue_reputation(
        env: Env,
        contract_id: u32,
        caller: Address,
        freelancer: Address,
        rating: i128,
    ) -> bool {
        Self::require_not_paused(&env);

        let contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));
        ttl::extend_contract_ttl(&env, contract_id);

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

    /// Returns a bounded page of contract ids that the given `participant` is involved in.
    ///
    /// This is an efficient alternative to client-side scanning of all contract ids.
    ///
    /// # Arguments
    /// * `env` - Soroban environment
    /// * `participant` - Address to list contracts for
    /// * `role` - 0 => `participant` is the client, 1 => `participant` is the freelancer
    /// * `start` - 0-based start index into the participant's id list
    /// * `limit` - maximum number of ids to return (capped by an internal maximum)
    ///
    /// # Returns
    /// A vector of contract ids in creation order.
    ///
    /// # Security
    /// - Read-only: does not mutate storage.
    /// - Limit-bounded to prevent unbounded reads.
    pub fn list_contracts_by_participant(
        env: Env,
        participant: Address,
        role: u8,
        start: u32,
        limit: u32,
    ) -> Vec<u32> {
        // Hard cap to avoid unbounded reads.
        const MAX_LIMIT: u32 = 50;

        if limit == 0 {
            return Vec::new(&env);
        }

        let effective_limit = if limit > MAX_LIMIT { MAX_LIMIT } else { limit };

        let key = match role {
            0 => DataKey::ClientContracts(participant),
            1 => DataKey::FreelancerContracts(participant),
            _ => env.panic_with_error(Error::InvalidParticipants),
        };

        let ids: Vec<u32> = env.storage().persistent().get(&key).unwrap_or(Vec::new(&env));
        let total: u32 = ids.len();

        if start >= total {
            return Vec::new(&env);
        }

        let end_exclusive = core::cmp::min(start.saturating_add(effective_limit), total);
        let mut page: Vec<u32> = Vec::new(&env);
        let mut i: u32 = start;

        while i < end_exclusive {
            page.push_back(ids.get(i).unwrap());
            i += 1;
        }

        page
    }

    // -----------------------------------------------------------------------
    // Protocol fee readers
    // -----------------------------------------------------------------------

    /// Returns the current protocol fee rate in basis points (0–10 000).
    ///
    /// The value is written by [`set_protocol_fee_bps`] and read at milestone
    /// release time to calculate the fee deducted from each payment.
    ///
    /// No authentication required — this is a read-only view function.
    /// Bumps the persistent TTL on access so dashboards polling at low
    /// frequency cannot inadvertently let the entry expire.
    ///
    /// Returns `0` when no fee rate has been configured (i.e. the entry has
    /// never been written or the contract has not been initialized).
    pub fn get_protocol_fee_bps(env: Env) -> u32 {
        let key = DataKey::ProtocolFeeBps;
        let bps: u32 = env.storage().persistent().get(&key).unwrap_or(0);
        if env.storage().persistent().has(&key) {
            env.storage().persistent().extend_ttl(
                &key,
                ttl::PERSISTENT_BUMP_THRESHOLD,
                ttl::PERSISTENT_TTL_LEDGERS,
            );
        }
        bps
    }

    /// Returns the total protocol fees accumulated across all released
    /// milestones since the contract was last initialized, in the contract's
    /// native token stroops.
    ///
    /// The value is incremented in [`release_milestone`] and decremented by
    /// [`withdraw_protocol_fees`].  It is denominated in the same unit as
    /// milestone amounts (i128 stroops).
    ///
    /// No authentication required — this is a read-only view function.
    /// Bumps the persistent TTL on access.
    ///
    /// Returns `0` when no fees have been accumulated yet.
    pub fn get_accumulated_protocol_fees(env: Env) -> i128 {
        let key = DataKey::AccumulatedProtocolFees;
        let fees: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        if env.storage().persistent().has(&key) {
            env.storage().persistent().extend_ttl(
                &key,
                ttl::PERSISTENT_BUMP_THRESHOLD,
                ttl::PERSISTENT_TTL_LEDGERS,
            );
        }
        fees
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn require_initialized(env: &Env) {
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

#[cfg(test)]
mod test;
