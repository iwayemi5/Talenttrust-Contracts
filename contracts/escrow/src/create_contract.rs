use crate::{
    ttl, Contract, ContractStatus, DataKey, Error, Escrow, Milestone, ReleaseAuthorization,
};
use soroban_sdk::{symbol_short, Address, Env, Symbol, Vec};

impl Escrow {
    /// Core logic for creating a new escrow contract.
    ///
    /// Called from the single `#[contractimpl]` block in lib.rs after the
    /// initialization and pause guards have been checked.
    pub(crate) fn create_contract_impl(
        env: &Env,
        client: Address,
        freelancer: Address,
        arbiter: Option<Address>,
        milestones: Vec<i128>,
        release_authorization: ReleaseAuthorization,
    ) -> u32 {
        Self::require_not_paused(&env);
        client.require_auth();

        if client == freelancer {
            env.panic_with_error(Error::InvalidParticipants);
        }

        match release_authorization {
            ReleaseAuthorization::ArbiterOnly | ReleaseAuthorization::ClientAndArbiter
                if arbiter.is_none() =>
            {
                env.panic_with_error(Error::MissingArbiter);
            }
            _ => {}
        }

        if let Some(ref arb) = arbiter {
            if arb == &client || arb == &freelancer {
                env.panic_with_error(Error::InvalidArbiter);
            }
        }

        if milestones.is_empty() {
            env.panic_with_error(Error::EmptyMilestones);
        }

        // Enforce maximum number of milestones
        if milestones.len() > MAX_MILESTONES {
            env.panic_with_error(Error::TooManyMilestones);
        }

        let id = next_contract_id(env);

        // Write the counter before extending its TTL — extend_ttl panics if the key
        // has never been written.
        env.storage()
            .persistent()
            .set(&DataKey::NextContractId, &(id + 1));
        ttl::extend_next_contract_id_ttl(env);

        let freelancer_addr = freelancer.clone();
        let contract = Contract {
            client: client.clone(),
            freelancer: freelancer.clone(),
            arbiter,
            status: ContractStatus::Created,
            funded_amount: 0,
            released_amount: 0,
            refunded_amount: 0,
            release_authorization,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Contract(id), &contract);

        let mut milestone_vec: Vec<Milestone> = Vec::new(env);
        for amount in milestones.iter() {
            milestone_vec.push_back(Milestone {
                amount,
                funded_amount: 0,
                released: false,
                refunded: false,
                work_evidence: None,
                refunded_amount: 0,
                deadline: None,
            });
        }
        let milestone_key = Symbol::new(env, "milestones");
        env.storage()
            .persistent()
            .set(&(DataKey::Contract(id), milestone_key), &milestone_vec);

        // Track pending reputation credit for the freelancer.
        let pending_key = DataKey::PendingReputationCredits(freelancer_addr.clone());
        let pending: i128 = env.storage().persistent().get(&pending_key).unwrap_or(0);
        env.storage().persistent().set(&pending_key, &(pending + 1));

        env.events().publish(
            (symbol_short!("created"), id),
            (client, freelancer_addr, env.ledger().timestamp()),
        );

        id
    }

/// Returns the next contract id after verifying the slot is unused.
pub(crate) fn next_contract_id(env: &Env) -> u32 {
    let id: u32 = env
        .storage()
        .persistent()
        .get(&DataKey::NextContractId)
        .unwrap_or(1);

        if env
            .storage()
            .persistent()
            .get::<_, Contract>(&DataKey::Contract(id))
            .is_some()
        {
            env.panic_with_error(Error::ContractIdCollision);
        }

        id
    }

    id
}
