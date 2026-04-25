#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Bytes, BytesN, Env,
    Symbol, Vec,
};

mod ttl;

pub use ttl::{
    LEDGERS_PER_DAY, PENDING_APPROVAL_BUMP_THRESHOLD, PENDING_APPROVAL_TTL_LEDGERS,
    PENDING_MIGRATION_BUMP_THRESHOLD, PENDING_MIGRATION_TTL_LEDGERS,
};

use types::ContractStatus;

mod types;

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

mod types;
pub use crate::types::{MainnetReadinessInfo, ReadinessChecklist};
use crate::types::DataKey as ReadinessDataKey;

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
    UnauthorizedRole = 6,
    InvalidStatusTransition = 7,
    AlreadyCancelled = 8,
    ContractNotFound = 9,
    MilestonesAlreadyReleased = 10,
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingClientMigration {
    pub current_client: Address,
    pub proposed_client: Address,
    pub proposed_client_confirmed: bool,
    pub requested_at_ledger: u32,
    pub expires_at_ledger: u32,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Contract(u32),
    MilestoneReleased(u32, u32),
    RefundableBalance(u32),
    PendingClientMigration(u32),
}

fn update_readiness_checklist<F>(env: &Env, f: F)
where
    F: FnOnce(&mut ReadinessChecklist),
{
    let mut checklist: ReadinessChecklist = env
        .storage()
        .instance()
        .get(&ReadinessDataKey::ReadinessChecklist)
        .unwrap_or_default();
    f(&mut checklist);
    env.storage()
        .instance()
        .set(&ReadinessDataKey::ReadinessChecklist, &checklist);
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
        milestones: Vec<i128>,
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

        if milestones.is_empty() {
            env.panic_with_error(EscrowError::EmptyMilestones);
        }
        if milestones.len() > MAX_MILESTONES {
            env.panic_with_error(EscrowError::TooManyMilestones);
        }

        let mut total_amount: i128 = 0;
        let mut milestones: Vec<Milestone> = Vec::new(&env);
        for amount in milestone_amounts.iter() {
            if amount <= 0 {
                env.panic_with_error(EscrowError::InvalidMilestoneAmount);
            }
            total_amount += amount;
            milestones.push_back(Milestone {
                amount,
                released: false,
                refunded: false,
            });
        }

        let id: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::ContractCount)
            .unwrap_or(0u32);

        let data = EscrowContractData {
            client,
            freelancer,
            arbiter,
            milestones,
            status: ContractStatus::Created,
            total_deposited: 0,
            released_amount: 0,
        };

        env.storage().persistent().set(&DataKey::Contract(id), &data);
        env.storage()
            .persistent()
            .set(&DataKey::Milestones(id), &milestones);
        env.storage().persistent().set(&DataKey::ContractCount, &(id + 1));

        id
    }

    pub fn deposit_funds(env: Env, contract_id: u32, amount: i128) -> bool {
        if amount <= 0 {
            env.panic_with_error(EscrowError::InvalidDepositAmount);
        }

        let contract_key = DataKey::Contract(contract_id);
        let mut contract = env
            .storage()
            .persistent()
            .get::<_, ContractData>(&contract_key)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));

        contract.total_deposited += amount;

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
            .get::<_, ContractData>(&contract_key)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));

        // Mark this milestone as released
        let milestone_key = DataKey::MilestoneReleased(contract_id, milestone_index);
        env.storage().persistent().set(&milestone_key, &true);

        // Update released amount
        if let Some(amount) = contract.milestones.get(milestone_index) {
            contract.released_amount += amount;
        }

        env.storage().persistent().set(&contract_key, &contract);

        true
    }

    /// Get contract details
    pub fn get_contract(env: Env, contract_id: u32) -> ContractData {
        env.storage()
            .persistent()
            .get::<_, ContractData>(&DataKey::Contract(contract_id))
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
            .get::<_, ContractData>(&contract_key)
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
    fn calculate_released_amount(env: &Env, contract_id: u32, contract: &ContractData) -> i128 {
        let mut released = 0i128;
        for (idx, amount) in contract.milestones.iter().enumerate() {
            let milestone_key = DataKey::MilestoneReleased(contract_id, idx as u32);
            if env
                .storage()
                .persistent()
                .get::<_, bool>(&milestone_key)
                .unwrap_or(false)
            {
                released += amount;
            }
        }
        released
    }

    /// Request client migration to a new address
    pub fn request_client_migration(env: Env, contract_id: u32, proposed_client: Address) -> bool {
        proposed_client.require_auth();

        let contract_key = DataKey::Contract(contract_id);
        let contract = env
            .storage()
            .persistent()
            .get::<_, EscrowContractData>(&contract_key)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));

        // Only current client can request migration
        let current_client = contract.client;
        current_client.require_auth();

        // Check if contract is in a state that allows migration
        if !Self::can_migrate_client(&contract.status) {
            env.panic_with_error(EscrowError::InvalidStatusTransition);
        }

        // Check if there's already a pending migration
        if Self::has_pending_client_migration_internal(&env, contract_id) {
            env.panic_with_error(EscrowError::AlreadyCancelled); // Reuse error for "already pending"
        }

        // Cannot migrate to same address
        if current_client == proposed_client {
            env.panic_with_error(EscrowError::InvalidParticipant);
        }

        // Create pending migration
        let current_ledger = env.ledger().sequence();
        let expires_at = current_ledger + PENDING_MIGRATION_TTL_LEDGERS;
        
        let pending_migration = PendingClientMigration {
            current_client: current_client.clone(),
            proposed_client: proposed_client.clone(),
            proposed_client_confirmed: false,
            requested_at_ledger: current_ledger,
            expires_at_ledger: expires_at,
        };

        env.storage()
            .persistent()
            .set(&DataKey::PendingClientMigration(contract_id), &pending_migration);

        // Emit event
        env.events().publish(
            (Symbol::new(&env, "client_migration_proposed"), contract_id),
            (current_client, proposed_client, current_ledger),
        );

        true
    }

    /// Confirm client migration by the proposed client
    pub fn confirm_client_migration(env: Env, contract_id: u32) -> bool {
        let pending_key = DataKey::PendingClientMigration(contract_id);
        let mut pending = env
            .storage()
            .persistent()
            .get::<_, PendingClientMigration>(&pending_key)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));

        // Proposed client must confirm
        pending.proposed_client.require_auth();

        // Check if migration is still valid (not expired)
        let current_ledger = env.ledger().sequence();
        if current_ledger > pending.expires_at_ledger {
            // Remove expired migration
            env.storage().persistent().remove(&pending_key);
            env.panic_with_error(EscrowError::InvalidStatusTransition);
        }

        // Mark as confirmed
        pending.proposed_client_confirmed = true;
        env.storage().persistent().set(&pending_key, &pending);

        // Emit event
        env.events().publish(
            (Symbol::new(&env, "client_migration_confirmed"), contract_id),
            (pending.current_client, pending.proposed_client, current_ledger),
        );

        true
    }

    /// Finalize client migration (atomic update)
    pub fn finalize_client_migration(env: Env, contract_id: u32) -> bool {
        let pending_key = DataKey::PendingClientMigration(contract_id);
        let pending = env
            .storage()
            .persistent()
            .get::<_, PendingClientMigration>(&pending_key)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));

        // Check if migration is confirmed and not expired
        if !pending.proposed_client_confirmed {
            env.panic_with_error(EscrowError::InvalidStatusTransition);
        }

        let current_ledger = env.ledger().sequence();
        if current_ledger > pending.expires_at_ledger {
            // Remove expired migration
            env.storage().persistent().remove(&pending_key);
            env.panic_with_error(EscrowError::InvalidStatusTransition);
        }

        // Update contract client atomically
        let contract_key = DataKey::Contract(contract_id);
        let mut contract = env
            .storage()
            .persistent()
            .get::<_, EscrowContractData>(&contract_key)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));

        contract.client = pending.proposed_client.clone();
        env.storage().persistent().set(&contract_key, &contract);

        // Remove pending migration
        env.storage().persistent().remove(&pending_key);

        // Emit event
        env.events().publish(
            (Symbol::new(&env, "client_migration_finalized"), contract_id),
            (pending.current_client, pending.proposed_client, current_ledger),
        );

        true
    }

    /// Cancel pending client migration
    pub fn cancel_client_migration(env: Env, contract_id: u32) -> bool {
        let pending_key = DataKey::PendingClientMigration(contract_id);
        let pending = env
            .storage()
            .persistent()
            .get::<_, PendingClientMigration>(&pending_key)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));

        // Only current client can cancel
        pending.current_client.require_auth();

        // Remove pending migration
        env.storage().persistent().remove(&pending_key);

        // Emit event
        env.events().publish(
            (Symbol::new(&env, "client_migration_cancelled"), contract_id),
            (pending.current_client, pending.proposed_client, env.ledger().sequence()),
        );

        true
    }

    /// Get pending client migration information
    pub fn get_pending_client_migration(env: Env, contract_id: u32) -> PendingClientMigration {
        env.storage()
            .persistent()
            .get::<_, PendingClientMigration>(&DataKey::PendingClientMigration(contract_id))
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound))
    }

    /// Check if there's a pending client migration
    pub fn has_pending_client_migration(env: Env, contract_id: u32) -> bool {
        Self::has_pending_client_migration_internal(&env, contract_id)
    }

    // Helper methods
    fn has_pending_client_migration_internal(env: &Env, contract_id: u32) -> bool {
        env.storage()
            .persistent()
            .get::<_, PendingClientMigration>(&DataKey::PendingClientMigration(contract_id))
            .is_some()
    }

    fn can_migrate_client(status: &ContractStatus) -> bool {
        match status {
            ContractStatus::Created | ContractStatus::Funded => true,
            ContractStatus::Completed | ContractStatus::Cancelled | ContractStatus::Disputed | ContractStatus::Refunded => false,
        }
    }
}

#[cfg(test)]
mod test;

#[cfg(test)]
mod proptest;
