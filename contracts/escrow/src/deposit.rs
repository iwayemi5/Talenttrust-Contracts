use crate::{ttl, Contract, ContractStatus, DataKey, Error, Escrow, Milestone};
use soroban_sdk::{Address, Env, Symbol, Vec};

impl Escrow {
    /// Core logic for depositing funds into a contract.
    ///
    /// Called from the single `#[contractimpl]` block in lib.rs after the
    /// initialization and pause guards have been checked.
    pub(crate) fn deposit_funds_impl(
        env: &Env,
        contract_id: u32,
        caller: Address,
        amount: i128,
    ) -> bool {
        if amount <= 0 {
            env.panic_with_error(Error::AmountMustBePositive);
        }

        // 2. Load the contract; bump TTL on the read path.
        let mut contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));

        ttl::extend_contract_ttl(env, contract_id);

        // 3. Status gate: only `Created` and `PartiallyFunded` accept deposits.
        //    Checking status before auth avoids leaking caller-info via auth
        //    for contracts that are no longer accepting deposits.
        match contract.status {
            ContractStatus::Created | ContractStatus::PartiallyFunded => {}
            _ => env.panic_with_error(Error::InvalidState),
        }

        // 4. Authenticate caller. Caller must be the stored client.
        if caller != contract.client {
            env.panic_with_error(Error::UnauthorizedRole);
        }
        caller.require_auth();

        if contract.status != ContractStatus::Created
            && contract.status != ContractStatus::PartiallyFunded
        {
            env.panic_with_error(Error::InvalidState);
        }

        contract.funded_amount += amount;

        let milestone_key = Symbol::new(env, "milestones");
        let milestones: Vec<Milestone> = env
            .storage()
            .persistent()
            .get(&(DataKey::Contract(contract_id), milestone_key.clone()))
            .unwrap();

        ttl::extend_milestone_ttl(env, contract_id);

        let total_amount: i128 = milestones.iter().map(|m| m.amount).sum();

        if contract.funded_amount >= total_amount {
            contract.status = ContractStatus::Funded;
        } else if contract.funded_amount > 0 && contract.status == ContractStatus::Created {
            contract.status = ContractStatus::PartiallyFunded;
        }

        // 7. Apply the deposit and transition the status machine.
        contract.funded_amount = new_funded;
        contract.status = if new_funded >= total_amount {
            ContractStatus::Funded
        } else {
            ContractStatus::PartiallyFunded
        };

        // 8. Persist milestones + contract, then extend TTL.
        env.storage().persistent().set(
            &(DataKey::Contract(contract_id), milestone_key),
            &milestones,
        );
        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);

        ttl::extend_contract_ttl(env, contract_id);

        // 9. Emit a structured deposit event so indexers can distinguish
        //    partial installments from final-funding deposits.
        env.events().publish(
            (symbol_short!("deposited"), contract_id),
            (
                caller,
                amount,
                contract.funded_amount,
                total_amount,
                contract.status.clone(),
            ),
        );

        true
    }
}
