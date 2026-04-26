#![no_std]

use soroban_sdk::{
    contract, contractimpl, symbol_short, vec, Address, Bytes, BytesN, Env, Symbol, Vec,
};

mod ttl;

pub use ttl::{
    LEDGERS_PER_DAY, PENDING_APPROVAL_BUMP_THRESHOLD, PENDING_APPROVAL_TTL_LEDGERS,
    PENDING_MIGRATION_BUMP_THRESHOLD, PENDING_MIGRATION_TTL_LEDGERS,
};

mod types;
pub use crate::types::{
    ContractStatus, DataKey, EscrowBounds, EscrowContractData, EscrowError, MainnetReadinessInfo,
    Milestone, ReadinessChecklist, ReputationRecord,
};

// ─── Bounds constants ─────────────────────────────────────────────────────────
pub const MAX_MILESTONES: u32 = 10;
pub const MAX_TOTAL_ESCROW_STROOPS: i128 = 10_000_000_000_000; // 1 M tokens × 10^7 = 10^13
pub const MAINNET_PROTOCOL_VERSION: u32 = 1u32;

#[contract]
pub struct Escrow;

#[contractimpl]
impl Escrow {
    pub fn hello(_env: Env, to: Symbol) -> Symbol {
        to
    }

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
        _terms_hash: Option<Bytes>,
        _grace_period_seconds: Option<u64>,
    ) -> u32 {
        client.require_auth();

        if client == freelancer {
            env.panic_with_error(EscrowError::InvalidParticipant);
        }

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

        let mut total_amount: i128 = 0;
        let mut milestones: Vec<Milestone> = Vec::new(&env);
        for amount in milestone_amounts.iter() {
            if amount <= 0 {
                env.panic_with_error(EscrowError::InvalidMilestoneAmount);
            }
            total_amount += amount;
            milestones.push_back(Milestone {
                amount,
                funded_amount: 0,
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
            client: client.clone(),
            freelancer: freelancer.clone(),
            arbiter,
            milestones: milestones.clone(),
            status: ContractStatus::Created,
            total_amount,
            funded_amount: 0,
            released_amount: 0,
            refunded_amount: 0,
            finalized: false,
            terms_hash: _terms_hash,
            grace_period_seconds: _grace_period_seconds,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Contract(id), &data);
        env.storage()
            .persistent()
            .set(&DataKey::ContractCount, &(id + 1));

        env.events().publish(
            (symbol_short!("created"), id),
            (client, freelancer, total_amount),
        );

        id
    }

    pub fn deposit_funds(env: Env, contract_id: u32, amount: i128) -> bool {
        let mut contract = Self::get_contract(env.clone(), contract_id);
        contract.client.require_auth();

        if contract.status == ContractStatus::Completed
            || contract.status == ContractStatus::Cancelled
            || contract.status == ContractStatus::Refunded
        {
            env.panic_with_error(EscrowError::InvalidStatusTransition);
        }

        if amount <= 0 {
            env.panic_with_error(EscrowError::InvalidDepositAmount);
        }

        contract.funded_amount += amount;

        if contract.status == ContractStatus::Created {
            contract.status = ContractStatus::Funded;
        }

        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);

        env.events().publish(
            (symbol_short!("deposited"), contract_id),
            (amount, contract.client),
        );

        true
    }

    pub fn approve_milestone(env: Env, contract_id: u32, milestone_index: u32) -> bool {
        let contract = Self::get_contract(env.clone(), contract_id);
        contract.client.require_auth();

        let approval_time = env.ledger().timestamp();
        env.storage().persistent().set(
            &DataKey::MilestoneApprovalTime(contract_id, milestone_index),
            &approval_time,
        );

        env.events()
            .publish((symbol_short!("approved"), contract_id), milestone_index);

        true
    }

    pub fn release_milestone(env: Env, contract_id: u32, milestone_index: u32) -> bool {
        let mut contract = Self::get_contract(env.clone(), contract_id);

        // Auth: Client or Arbiter
        let is_arbiter = contract.arbiter.as_ref().is_some_and(|a| {
            a.require_auth();
            true
        });
        if !is_arbiter {
            contract.client.require_auth();
        }

        if milestone_index >= contract.milestones.len() {
            env.panic_with_error(EscrowError::InvalidMilestone);
        }

        let mut milestones = contract.milestones.clone();
        let mut milestone = milestones.get(milestone_index).unwrap();

        if milestone.released {
            env.panic_with_error(EscrowError::AlreadyReleased);
        }
        if milestone.refunded {
            env.panic_with_error(EscrowError::InvalidStatusTransition); // Cannot release refunded
        }

        if contract.funded_amount - contract.released_amount - contract.refunded_amount
            < milestone.amount
        {
            env.panic_with_error(EscrowError::InsufficientFunds);
        }

        milestone.released = true;
        milestones.set(milestone_index, milestone.clone());
        contract.milestones = milestones;
        contract.released_amount += milestone.amount;

        // Check if all milestones released or refunded
        let mut all_done = true;
        for ms in contract.milestones.iter() {
            if !ms.released && !ms.refunded {
                all_done = false;
                break;
            }
        }

        if all_done {
            contract.status = ContractStatus::Completed;

            // Increment pending reputation only if some milestones were released
            if contract.released_amount > 0 {
                let mut pending: u32 = env
                    .storage()
                    .persistent()
                    .get(&DataKey::PendingReputation(contract.freelancer.clone()))
                    .unwrap_or(0);
                pending += 1;
                env.storage().persistent().set(
                    &DataKey::PendingReputation(contract.freelancer.clone()),
                    &pending,
                );
            }

            env.events()
                .publish((symbol_short!("completed"), contract_id), ());
        }

        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);

        env.events().publish(
            (symbol_short!("released"), contract_id),
            (milestone_index, milestone.amount),
        );

        true
    }

    pub fn refund_unreleased_milestones(
        env: Env,
        contract_id: u32,
        milestone_indices: Vec<u32>,
    ) -> i128 {
        let mut contract = Self::get_contract(env.clone(), contract_id);

        // Only Arbiter can trigger refunds (based on docs/PR description)
        if let Some(ref a) = contract.arbiter {
            a.require_auth();
        } else {
            env.panic_with_error(EscrowError::UnauthorizedRole);
        }

        let mut total_refunded = 0i128;
        let mut milestones = contract.milestones.clone();

        for idx in milestone_indices.iter() {
            if idx >= milestones.len() {
                env.panic_with_error(EscrowError::InvalidMilestone);
            }
            let mut ms = milestones.get(idx).unwrap();
            if ms.released {
                env.panic_with_error(EscrowError::AlreadyReleased);
            }
            if ms.refunded {
                env.panic_with_error(EscrowError::InvalidStatusTransition);
            }
            ms.refunded = true;
            total_refunded += ms.amount;
            milestones.set(idx, ms);
        }

        if contract.funded_amount - contract.released_amount - contract.refunded_amount
            < total_refunded
        {
            env.panic_with_error(EscrowError::InsufficientFunds);
        }

        contract.milestones = milestones;
        contract.refunded_amount += total_refunded;

        // Check if all milestones are done
        let mut all_done = true;
        for ms in contract.milestones.iter() {
            if !ms.released && !ms.refunded {
                all_done = false;
                break;
            }
        }

        if all_done {
            contract.status = ContractStatus::Refunded;

            // Increment pending reputation if some milestones were released before final refund
            if contract.released_amount > 0 {
                let mut pending: u32 = env
                    .storage()
                    .persistent()
                    .get(&DataKey::PendingReputation(contract.freelancer.clone()))
                    .unwrap_or(0);
                pending += 1;
                env.storage().persistent().set(
                    &DataKey::PendingReputation(contract.freelancer.clone()),
                    &pending,
                );
            }

            env.events()
                .publish((symbol_short!("refunded"), contract_id), total_refunded);
        }

        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);

        total_refunded
    }

    pub fn finalize_contract(env: Env, contract_id: u32) -> bool {
        let mut contract = Self::get_contract(env.clone(), contract_id);
        contract.client.require_auth();

        if contract.finalized {
            env.panic_with_error(EscrowError::AlreadyFinalized);
        }

        if contract.status != ContractStatus::Completed
            && contract.status != ContractStatus::Cancelled
            && contract.status != ContractStatus::Refunded
        {
            env.panic_with_error(EscrowError::NotReadyForFinalization);
        }

        contract.finalized = true;
        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);

        env.events()
            .publish((symbol_short!("finalized"), contract_id), ());

        true
    }

    pub fn withdraw_leftover(env: Env, contract_id: u32, caller: Address) -> i128 {
        caller.require_auth();
        let mut contract = Self::get_contract(env.clone(), contract_id);

        if caller != contract.client {
            env.panic_with_error(EscrowError::UnauthorizedRole);
        }

        if !contract.finalized {
            env.panic_with_error(EscrowError::NotReadyForFinalization);
        }

        let leftover = contract.funded_amount - contract.released_amount - contract.refunded_amount;
        if leftover <= 0 {
            env.panic_with_error(EscrowError::InvalidDepositAmount);
        }

        contract.funded_amount -= leftover;
        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);

        env.events().publish(
            (symbol_short!("withdrawn"), contract_id),
            (leftover, caller),
        );

        leftover
    }

    pub fn issue_reputation(env: Env, contract_id: u32, rating: u32) -> bool {
        let contract = Self::get_contract(env.clone(), contract_id);
        contract.client.require_auth();

        if contract.status != ContractStatus::Completed
            && contract.status != ContractStatus::Refunded
        {
            env.panic_with_error(EscrowError::NotCompleted);
        }

        if !(1..=5).contains(&rating) {
            env.panic_with_error(EscrowError::InvalidRating);
        }

        if env.storage().persistent().has(&DataKey::Reputation(
            contract_id,
            contract.freelancer.clone(),
        )) {
            env.panic_with_error(EscrowError::DuplicateRating);
        }

        let mut pending: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::PendingReputation(contract.freelancer.clone()))
            .unwrap_or(0);
        if pending == 0 {
            env.panic_with_error(EscrowError::NotCompleted);
        }
        pending -= 1;
        env.storage().persistent().set(
            &DataKey::PendingReputation(contract.freelancer.clone()),
            &pending,
        );

        let mut record: ReputationRecord = env
            .storage()
            .persistent()
            .get(&DataKey::ReputationRecord(contract.freelancer.clone()))
            .unwrap_or(ReputationRecord {
                completed_contracts: 0,
                total_rating: 0,
                last_rating: 0,
                ratings_count: 0,
            });

        record.completed_contracts += 1;
        record.total_rating += rating;
        record.last_rating = rating;
        record.ratings_count += 1;

        env.storage().persistent().set(
            &DataKey::ReputationRecord(contract.freelancer.clone()),
            &record,
        );
        env.storage().persistent().set(
            &DataKey::Reputation(contract_id, contract.freelancer.clone()),
            &rating,
        );

        env.events().publish(
            (symbol_short!("rated"), contract_id),
            (contract.freelancer, rating),
        );

        true
    }

    pub fn get_pending_reputation_credits(env: Env, freelancer: Address) -> u32 {
        env.storage()
            .persistent()
            .get(&DataKey::PendingReputation(freelancer))
            .unwrap_or(0)
    }

    pub fn get_reputation(env: Env, freelancer: Address) -> Option<ReputationRecord> {
        env.storage()
            .persistent()
            .get(&DataKey::ReputationRecord(freelancer))
    }

    pub fn get_contract(env: Env, contract_id: u32) -> EscrowContractData {
        env.storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound))
    }

    pub fn get_milestones(env: Env, contract_id: u32) -> Vec<Milestone> {
        let contract = Self::get_contract(env, contract_id);
        contract.milestones
    }

    pub fn get_terms_hash(env: Env, contract_id: u32) -> Option<Bytes> {
        let contract = Self::get_contract(env.clone(), contract_id);
        contract.terms_hash
    }

    pub fn get_grace_period(env: Env, contract_id: u32) -> Option<u64> {
        let contract = Self::get_contract(env.clone(), contract_id);
        contract.grace_period_seconds
    }

    pub fn get_milestone_approval_time(
        env: Env,
        contract_id: u32,
        milestone_index: u32,
    ) -> Option<u64> {
        env.storage()
            .persistent()
            .get(&DataKey::MilestoneApprovalTime(
                contract_id,
                milestone_index,
            ))
    }

    pub fn set_milestone_funded(
        env: Env,
        contract_id: u32,
        milestone_index: u32,
        amount: i128,
    ) -> bool {
        let mut contract = Self::get_contract(env.clone(), contract_id);
        contract.client.require_auth();

        if milestone_index >= contract.milestones.len() {
            env.panic_with_error(EscrowError::InvalidMilestone);
        }

        let mut milestones = contract.milestones.clone();
        let mut milestone = milestones.get(milestone_index).unwrap();

        milestone.funded_amount = amount;
        milestones.set(milestone_index, milestone);
        contract.milestones = milestones;

        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);
        true
    }

    pub fn get_milestone_funded(env: Env, contract_id: u32, milestone_index: u32) -> i128 {
        let contract = Self::get_contract(env.clone(), contract_id);
        if milestone_index >= contract.milestones.len() {
            env.panic_with_error(EscrowError::InvalidMilestone);
        }
        contract
            .milestones
            .get(milestone_index)
            .unwrap()
            .funded_amount
    }

    pub fn get_refundable_balance(env: Env, contract_id: u32) -> i128 {
        let contract = Self::get_contract(env.clone(), contract_id);
        contract.funded_amount - contract.released_amount - contract.refunded_amount
    }

    pub fn dispute_contract(env: Env, contract_id: u32, caller: Address) -> bool {
        caller.require_auth();
        let mut contract = Self::get_contract(env.clone(), contract_id);

        if contract.status != ContractStatus::Funded {
            env.panic_with_error(EscrowError::InvalidStatusTransition);
        }

        let is_client = caller == contract.client;
        let is_freelancer = caller == contract.freelancer;

        if !is_client && !is_freelancer {
            env.panic_with_error(EscrowError::UnauthorizedRole);
        }

        contract.status = ContractStatus::Disputed;
        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);

        env.events()
            .publish((symbol_short!("disputed"), contract_id), caller);

        true
    }

    pub fn refund(env: Env, contract_id: u32, milestone_index: u32) -> i128 {
        let indices = vec![&env, milestone_index];
        Self::refund_unreleased_milestones(env.clone(), contract_id, indices)
    }

    pub fn cancel(env: Env, contract_id: u32) -> bool {
        let contract = Self::get_contract(env.clone(), contract_id);
        Self::cancel_contract(env.clone(), contract_id, contract.client)
    }

    pub fn dispute(env: Env, contract_id: u32) -> bool {
        let contract = Self::get_contract(env.clone(), contract_id);
        Self::dispute_contract(env.clone(), contract_id, contract.client)
    }

    pub fn request_approval(
        env: Env,
        approver: Address,
        contract_id: u32,
    ) -> crate::types::PendingApproval {
        approver.require_auth();
        if ttl::has_transient(&env, &DataKey::PendingApproval(contract_id)) {
            env.panic_with_error(EscrowError::UnauthorizedRole); // Using role for "already exists" for now
        }

        let requested_at = env.ledger().sequence();
        let expires_at = requested_at + PENDING_APPROVAL_TTL_LEDGERS;

        let pending = crate::types::PendingApproval {
            approver: approver.clone(),
            contract_id,
            requested_at_ledger: requested_at,
            expires_at_ledger: expires_at,
        };

        ttl::store_with_ttl(
            &env,
            &DataKey::PendingApproval(contract_id),
            &pending,
            PENDING_APPROVAL_TTL_LEDGERS,
        );
        pending
    }

    pub fn get_pending_approval(
        env: Env,
        contract_id: u32,
    ) -> Option<crate::types::PendingApproval> {
        ttl::read_if_live(&env, &DataKey::PendingApproval(contract_id))
    }

    pub fn extend_pending_approval(env: Env, approver: Address, contract_id: u32) -> bool {
        approver.require_auth();
        ttl::extend_if_below_threshold(
            &env,
            &DataKey::PendingApproval(contract_id),
            PENDING_APPROVAL_BUMP_THRESHOLD,
            PENDING_APPROVAL_TTL_LEDGERS,
        )
    }

    pub fn cancel_approval(env: Env, approver: Address, contract_id: u32) {
        approver.require_auth();
        ttl::remove_transient(&env, &DataKey::PendingApproval(contract_id));
    }

    pub fn request_migration(
        env: Env,
        proposer: Address,
        wasm_hash: BytesN<32>,
    ) -> crate::types::PendingMigration {
        proposer.require_auth();
        if ttl::has_transient(&env, &DataKey::PendingMigration) {
            env.panic_with_error(EscrowError::AlreadyCancelled); // Using for "already exists"
        }

        let requested_at = env.ledger().sequence();
        let expires_at = requested_at + PENDING_MIGRATION_TTL_LEDGERS;

        let pending = crate::types::PendingMigration {
            proposer: proposer.clone(),
            new_wasm_hash: wasm_hash,
            requested_at_ledger: requested_at,
            expires_at_ledger: expires_at,
        };

        ttl::store_with_ttl(
            &env,
            &DataKey::PendingMigration,
            &pending,
            PENDING_MIGRATION_TTL_LEDGERS,
        );
        pending
    }

    pub fn get_pending_migration(env: Env) -> Option<crate::types::PendingMigration> {
        ttl::read_if_live(&env, &DataKey::PendingMigration)
    }

    pub fn confirm_migration(env: Env, confirmer: Address) {
        confirmer.require_auth();
        ttl::remove_transient(&env, &DataKey::PendingMigration);
        // In real app, this would trigger actual migration
    }

    pub fn cancel_contract(env: Env, contract_id: u32, caller: Address) -> bool {
        caller.require_auth();
        let mut contract = Self::get_contract(env.clone(), contract_id);

        if contract.status == ContractStatus::Cancelled {
            env.panic_with_error(EscrowError::AlreadyCancelled);
        }
        if contract.status == ContractStatus::Completed {
            env.panic_with_error(EscrowError::InvalidStatusTransition);
        }

        let is_client = caller == contract.client;
        let is_freelancer = caller == contract.freelancer;
        let is_arbiter = contract.arbiter.as_ref().is_some_and(|a| *a == caller);

        match contract.status {
            ContractStatus::Created => {
                if !is_client && !is_freelancer {
                    env.panic_with_error(EscrowError::UnauthorizedRole);
                }
            }
            ContractStatus::Funded => {
                if is_client {
                    if contract.released_amount > 0 {
                        env.panic_with_error(EscrowError::MilestonesAlreadyReleased);
                    }
                } else if !is_freelancer && !is_arbiter {
                    env.panic_with_error(EscrowError::UnauthorizedRole);
                }
            }
            ContractStatus::Disputed => {
                if !is_arbiter {
                    env.panic_with_error(EscrowError::UnauthorizedRole);
                }
            }
            _ => env.panic_with_error(EscrowError::InvalidStatusTransition),
        }

        contract.status = ContractStatus::Cancelled;
        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);

        env.events().publish(
            (symbol_short!("cancelled"), contract_id),
            (caller, contract.status, env.ledger().timestamp()),
        );

        true
    }
}

#[cfg(test)]
mod proptest;
#[cfg(test)]
mod test;
