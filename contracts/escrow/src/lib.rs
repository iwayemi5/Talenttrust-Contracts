#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Bytes, BytesN, Env,
    Symbol, Vec,
};

mod ttl;

pub use ttl::{
    LEDGERS_PER_DAY, PENDING_APPROVAL_BUMP_THRESHOLD, PENDING_APPROVAL_TTL_LEDGERS,
    PENDING_MIGRATION_BUMP_THRESHOLD, PENDING_MIGRATION_TTL_LEDGERS,
};

mod types;
mod amount_validation;
pub use amount_validation::{
    validate_single_amount, validate_milestone_amounts, validate_deposit_amount,
    validate_contract_total, safe_add_amounts, safe_subtract_amounts, AmountValidationError
};

use types::ContractStatus;
pub use crate::types::{
    CONTRACT_SUMMARY_SCHEMA_VERSION, ContractSummary, MilestoneSummary,
};

// ─── Bounds constants ─────────────────────────────────────────────────────────
//
// Policy decision: bounds are HARD-CODED for the initial release rather than
// governed on-chain. Rationale:
//   • Governance machinery adds upgrade-path complexity and new attack surface.
//   • Hard limits give the strongest security guarantee with zero runtime cost.
//   • A future governance proposal can introduce adjustable parameters if
//     operational experience shows the defaults need revisiting.
//
// MAX_MILESTONES: limits worst-case per-contract storage and loop cost.
//   10 milestones covers the overwhelming majority of real freelance contracts.
//
// MAX_TOTAL_ESCROW_STROOPS: caps the maximum value locked in a single contract
//   to 1 000 000 tokens (7-decimal stroops) to bound worst-case griefing impact.

/// Maximum number of milestones allowed per contract.
pub const MAX_MILESTONES: u32 = 10;

/// Hard cap on the total escrow value per contract, in stroops (7 decimal places).
/// Equals 1 000 000 tokens.
pub const MAX_TOTAL_ESCROW_STROOPS: i128 = 1_000_000_0000000; // 1 M tokens × 10^7 = 10^13

pub const MAINNET_PROTOCOL_VERSION: u32 = 1u32;
pub const MAINNET_MAX_TOTAL_ESCROW_PER_CONTRACT_STROOPS: i128 = 1_000_000_000_000_000i128;

#[contract]
pub struct Escrow;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowBounds {
    pub max_milestones: u32,
    pub max_total_escrow_stroops: i128,
}

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum EscrowError {
    InvalidParticipant = 1,
    EmptyMilestones = 2,
    InvalidMilestoneAmount = 3,
    InvalidDepositAmount = 4,
    InvalidMilestone = 5,
    UnauthorizedRole = 6,
    InvalidStatusTransition = 7,
    AlreadyCancelled = 8,
    ContractNotFound = 9,
    MilestonesAlreadyReleased = 10,
    TooManyMilestones = 11,
    // Amount validation errors (1000+ to avoid conflicts)
    NonPositiveAmount = 1000,
    AmountExceedsMaximum = 1001,
    PotentialOverflow = 1002,
    InvalidStroopPrecision = 1003,
    ExceedsContractMaximum = 1004,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowContractData {
    pub client: Address,
    pub freelancer: Address,
    pub arbiter: Option<Address>,
    pub milestones: Vec<i128>,
    pub status: ContractStatus,
    pub total_deposited: i128,
    pub released_amount: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingApproval {
    pub approver: Address,
    pub contract_id: u32,
    pub requested_at_ledger: u32,
    pub expires_at_ledger: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingMigration {
    pub proposer: Address,
    pub new_wasm_hash: BytesN<32>,
    pub requested_at_ledger: u32,
    pub expires_at_ledger: u32,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Contract(u32),
    MilestoneReleased(u32, u32),
    RefundableBalance(u32),
    ContractCount,
    MilestoneApprovalTime(u32, u32),
}


#[contractimpl]
impl Escrow {
    pub fn hello(_env: Env, to: Symbol) -> Symbol {
        to
    }

    /// Returns the hard-coded bounds enforced by this contract.
    /// Useful for client-side pre-validation and monitoring dashboards.
    pub fn get_bounds(_env: Env) -> EscrowBounds {
        EscrowBounds {
            max_milestones: MAX_MILESTONES,
            max_total_escrow_stroops: MAX_TOTAL_ESCROW_STROOPS,
        }
    }

    pub fn create_contract(
        env: Env,
        client: Address,
        freelancer: Address,
        arbiter: Option<Address>,
        milestone_amounts: Vec<i128>,
        terms_hash: Option<Bytes>,
        grace_period_seconds: Option<u64>,
    ) -> u32 {
        client.require_auth();

        if client == freelancer {
            env.panic_with_error(EscrowError::InvalidParticipant);
        }

        // Validate arbiter doesn't overlap with client/freelancer
        if let Some(ref a) = arbiter {
            if *a == client || *a == freelancer {
                env.panic_with_error(EscrowError::InvalidParticipant);
            }
        }

        if milestone_amounts.is_empty() {
            env.panic_with_error(EscrowError::EmptyMilestones);
        }
        if milestone_amounts.len() > MAX_MILESTONES {
            env.panic_with_error(EscrowError::TooManyMilestones);
        }

        // Use centralized amount validation for milestones
        // Validate each milestone amount individually and calculate total
        let mut total_amount: i128 = 0;
        for i in 0..milestone_amounts.len() {
            let amount = milestone_amounts.get(i).unwrap();
            validate_single_amount(amount).unwrap_or_else(|e| {
                match e {
                    AmountValidationError::NonPositiveAmount => 
                        env.panic_with_error(EscrowError::InvalidMilestoneAmount),
                    AmountValidationError::AmountExceedsMaximum => 
                        env.panic_with_error(EscrowError::InvalidMilestoneAmount),
                    AmountValidationError::PotentialOverflow => 
                        env.panic_with_error(EscrowError::InvalidMilestoneAmount),
                    AmountValidationError::InvalidStroopPrecision => 
                        env.panic_with_error(EscrowError::InvalidMilestoneAmount),
                    AmountValidationError::ExceedsContractMaximum => 
                        env.panic_with_error(EscrowError::InvalidMilestoneAmount),
                }
            });
            
            // Use safe addition to prevent overflow
            total_amount = safe_add_amounts(total_amount, amount)
                .unwrap_or_else(|| env.panic_with_error(EscrowError::PotentialOverflow));
        }
        
        // Validate total against contract maximum
        validate_contract_total(total_amount, MAX_TOTAL_ESCROW_STROOPS)
            .unwrap_or_else(|e| {
                match e {
                    AmountValidationError::ExceedsContractMaximum => 
                        env.panic_with_error(EscrowError::InvalidMilestoneAmount),
                    _ => env.panic_with_error(EscrowError::InvalidMilestoneAmount),
                }
            });

        let id: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::ContractCount)
            .unwrap_or(0u32);

        let data = EscrowContractData {
            client,
            freelancer,
            arbiter,
            milestones: milestone_amounts,
            status: ContractStatus::Created,
            total_deposited: 0,
            released_amount: 0,
        };

        env.storage().persistent().set(&DataKey::Contract(id), &data);
        env.storage().persistent().set(&DataKey::ContractCount, &(id + 1));

        id
    }

    pub fn deposit_funds(env: Env, contract_id: u32, amount: i128) -> bool {
        // Use centralized amount validation for deposit
        validate_deposit_amount(amount, 0, MAX_TOTAL_ESCROW_STROOPS)
            .unwrap_or_else(|e| {
                // Convert amount validation errors to EscrowError
                match e {
                    AmountValidationError::NonPositiveAmount => 
                        env.panic_with_error(EscrowError::InvalidDepositAmount),
                    AmountValidationError::AmountExceedsMaximum => 
                        env.panic_with_error(EscrowError::InvalidDepositAmount),
                    AmountValidationError::PotentialOverflow => 
                        env.panic_with_error(EscrowError::InvalidDepositAmount),
                    AmountValidationError::ExceedsContractMaximum => 
                        env.panic_with_error(EscrowError::InvalidDepositAmount),
                    AmountValidationError::InvalidStroopPrecision => 
                        env.panic_with_error(EscrowError::InvalidDepositAmount),
                }
            });

        let contract_key = DataKey::Contract(contract_id);
        let mut contract = env
            .storage()
            .persistent()
            .get::<_, EscrowContractData>(&contract_key)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));

        // Additional validation: check against current deposited amount
        validate_deposit_amount(amount, contract.total_deposited, MAX_TOTAL_ESCROW_STROOPS)
            .unwrap_or_else(|e| {
                match e {
                    AmountValidationError::ExceedsContractMaximum => 
                        env.panic_with_error(EscrowError::InvalidDepositAmount),
                    _ => env.panic_with_error(EscrowError::InvalidDepositAmount),
                }
            });

        // Use safe addition to prevent overflow
        contract.total_deposited = safe_add_amounts(contract.total_deposited, amount)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::PotentialOverflow));

        // Update status to Funded if not already
        if contract.status == ContractStatus::Created {
            contract.status = ContractStatus::Funded;
        }

        env.storage().persistent().set(&contract_key, &contract);

        true
    }

    pub fn approve_milestone(env: Env, contract_id: u32, milestone_index: u32) -> bool {
        // Store approval time using ledger timestamp
        let approval_time = env.ledger().timestamp();
        env.storage().persistent().set(
            &DataKey::MilestoneApprovalTime(contract_id, milestone_index),
            &approval_time,
        );
        true
    }

    pub fn release_milestone(env: Env, contract_id: u32, milestone_index: u32) -> bool {
        let contract_key = DataKey::Contract(contract_id);
        let mut contract = env
            .storage()
            .persistent()
            .get::<_, EscrowContractData>(&contract_key)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));

        // Validate milestone index
        if milestone_index >= contract.milestones.len() {
            env.panic_with_error(EscrowError::InvalidMilestone);
        }

        // Mark this milestone as released
        let milestone_key = DataKey::MilestoneReleased(contract_id, milestone_index);
        env.storage().persistent().set(&milestone_key, &true);

        // Update released amount using safe arithmetic
        if let Some(amount) = contract.milestones.get(milestone_index) {
            contract.released_amount = safe_add_amounts(contract.released_amount, amount)
                .unwrap_or_else(|| env.panic_with_error(EscrowError::PotentialOverflow));
        }

        env.storage().persistent().set(&contract_key, &contract);

        true
    }

    /// Get contract details
    pub fn get_contract(env: Env, contract_id: u32) -> EscrowContractData {
        env.storage()
            .persistent()
            .get::<_, EscrowContractData>(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound))
    }

    /// Get milestones for a contract
    pub fn get_milestones(env: Env, contract_id: u32) -> Vec<i128> {
        let contract = Self::get_contract(env.clone(), contract_id);
        contract.milestones
    }

    /// Cancel an escrow contract under strict authorization and state constraints
    pub fn cancel_contract(env: Env, contract_id: u32, caller: Address) -> bool {
        // 1. Require cryptographic authorization
        caller.require_auth();

        // 2. Load contract data
        let contract_key = DataKey::Contract(contract_id);
        let mut contract = env
            .storage()
            .persistent()
            .get::<_, EscrowContractData>(&contract_key)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));

        // 3. Check if already cancelled (idempotency guard)
        if contract.status == ContractStatus::Cancelled {
            env.panic_with_error(EscrowError::AlreadyCancelled);
        }

        // 4. Block cancellation in terminal states
        if contract.status == ContractStatus::Completed {
            env.panic_with_error(EscrowError::InvalidStatusTransition);
        }

        // 5. Role-based authorization with state checks
        let is_client = caller == contract.client;
        let is_freelancer = caller == contract.freelancer;
        let is_arbiter = contract.arbiter.as_ref().is_some_and(|a| *a == caller);

        match contract.status {
            ContractStatus::Created => {
                // Client or freelancer can cancel before funding
                if !is_client && !is_freelancer {
                    env.panic_with_error(EscrowError::UnauthorizedRole);
                }
            }
            ContractStatus::Funded => {
                // Calculate released milestones
                let released_amount = Self::calculate_released_amount(&env, contract_id, &contract);

                if is_client {
                    // Client can cancel only if NO milestones released
                    if released_amount > 0 {
                        env.panic_with_error(EscrowError::MilestonesAlreadyReleased);
                    }
                } else if is_freelancer {
                    // Freelancer can cancel (economic deterrent - funds return to client)
                    // No additional checks needed
                } else if is_arbiter {
                    // Arbiter can cancel in funded state (dispute resolution)
                } else {
                    env.panic_with_error(EscrowError::UnauthorizedRole);
                }
            }
            ContractStatus::Disputed => {
                // Only arbiter can cancel disputed contracts
                if !is_arbiter {
                    env.panic_with_error(EscrowError::UnauthorizedRole);
                }
            }
            _ => {
                env.panic_with_error(EscrowError::InvalidStatusTransition);
            }
        }

        // 6. Transition to Cancelled state
        contract.status = ContractStatus::Cancelled;
        env.storage().persistent().set(&contract_key, &contract);

        // 7. Emit indexer-friendly event
        env.events().publish(
            (Symbol::new(&env, "contract_cancelled"), contract_id),
            (caller, contract.status, env.ledger().timestamp()),
        );

        true
    }

    /// Helper: Calculate total released amount for a contract
    fn calculate_released_amount(env: &Env, contract_id: u32, contract: &EscrowContractData) -> i128 {
        let mut released = 0i128;
        for (idx, amount) in contract.milestones.iter().enumerate() {
            let milestone_key = DataKey::MilestoneReleased(contract_id, idx as u32);
            if env
                .storage()
                .persistent()
                .get::<_, bool>(&milestone_key)
                .unwrap_or(false)
            {
                released = safe_add_amounts(released, amount)
                    .unwrap_or_else(|| env.panic_with_error(EscrowError::PotentialOverflow));
            }
        }
        released
    }

    /// Returns a stable, single-read summary of an escrow contract for off-chain indexers.
    ///
    /// Combines contract roles, lifecycle status, financial totals, and
    /// per-milestone state into one atomic call so that indexing pipelines
    /// do not need multiple separate storage reads.
    ///
    /// # Fields
    ///
    /// | Field | Description |
    /// |---|---|
    /// | `schema_version` | Always `CONTRACT_SUMMARY_SCHEMA_VERSION` (`1`); incremented on breaking changes |
    /// | `client` | Address that funds the contract |
    /// | `freelancer` | Address that receives milestone payments |
    /// | `arbiter` | Optional dispute-resolution address (`None` if not set) |
    /// | `status` | Current lifecycle status (`Created`, `Funded`, `Completed`, `Cancelled`, `Refunded`, `Disputed`) |
    /// | `reputation_issued` | Whether a reputation score has already been recorded |
    /// | `total_amount` | Sum of all milestone amounts in stroops |
    /// | `funded_amount` | Total deposited by the client in stroops |
    /// | `released_amount` | Total released to the freelancer in stroops |
    /// | `refundable_balance` | Balance not yet released or refunded, in stroops |
    /// | `released_milestone_count` | Number of milestones released so far |
    /// | `milestones` | Per-milestone index, amount, `released`, and `refunded` flags |
    ///
    /// # Errors
    ///
    /// Panics with `EscrowError::ContractNotFound` if `contract_id` does not exist.
    ///
    /// # Backwards compatibility
    ///
    /// This method is additive and backwards-compatible with all existing
    /// contract storage.  If the return layout ever changes in a breaking way
    /// `CONTRACT_SUMMARY_SCHEMA_VERSION` will be incremented so consumers can
    /// detect and handle the new format.
    pub fn get_contract_summary(env: Env, contract_id: u32) -> ContractSummary {
        // Load the main contract record (panics with ContractNotFound if absent).
        let record = Self::get_contract(env.clone(), contract_id);

        // Load the ordered milestone list.
        let raw_milestones = Self::get_milestones(env.clone(), contract_id);

        // Load the current refundable balance (0 if never set).
        let refundable_balance = Self::get_refundable_balance(env.clone(), contract_id);

        // Build the per-milestone summaries and count released milestones.
        let mut milestone_summaries: Vec<MilestoneSummary> = Vec::new(&env);
        let mut released_milestone_count: u32 = 0u32;

        for (idx, m) in raw_milestones.iter().enumerate() {
            if m.released {
                released_milestone_count += 1;
            }
            milestone_summaries.push_back(MilestoneSummary {
                index: idx as u32,
                amount: m.amount,
                released: m.released,
                refunded: m.refunded,
            });
        }

        ContractSummary {
            schema_version: CONTRACT_SUMMARY_SCHEMA_VERSION,
            client: record.client,
            freelancer: record.freelancer,
            arbiter: record.arbiter,
            status: record.status,
            reputation_issued: record.reputation_issued,
            total_amount: record.total_amount,
            funded_amount: record.funded_amount,
            released_amount: record.released_amount,
            refundable_balance,
            released_milestone_count,
            milestones: milestone_summaries,
        }
    }
}

// #[cfg(test)]
// mod test;

// #[cfg(test)]
// mod proptest;

#[cfg(test)]
mod simple_amount_test;
