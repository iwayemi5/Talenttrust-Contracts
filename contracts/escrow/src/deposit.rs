use crate::{ttl, Contract, ContractStatus, DataKey, Error, Escrow};
use soroban_sdk::{contractimpl, Address, Env};

#[contractimpl]
impl Escrow {
    /// Deposits funds into the contract. Transitions to PartiallyFunded or Funded status.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    /// * `caller` - The address of the client making the deposit
    /// * `amount` - The amount to deposit (in stroops)
    ///
    /// # Returns
    /// `true` if deposit was successful
    ///
    /// # Errors
    /// * `AmountMustBePositive` - If amount is <= 0
    /// * `ContractNotFound` - If contract doesn't exist
    /// * `InvalidState` - If contract is not in Created or PartiallyFunded state
    /// * `UnauthorizedRole` - If caller is not the client
    pub fn deposit_funds(env: Env, contract_id: u32, caller: Address, amount: i128) -> bool {
        Self::require_not_paused(&env);
        if amount <= 0 {
            env.panic_with_error(Error::AmountMustBePositive);
        }

        let mut contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));

        ttl::extend_contract_ttl(&env, contract_id);

        Self::require_not_finalized(&env, contract_id);

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

        let milestones = ttl::load_milestones(&env, contract_id);

        let total_amount: i128 = milestones.iter().map(|m| m.amount).sum();

        if contract.funded_amount > total_amount {
            env.panic_with_error(Error::InvalidDepositAmount);
        }

        if contract.funded_amount >= total_amount {
            contract.status = ContractStatus::Funded;
        } else {
            contract.status = ContractStatus::PartiallyFunded;
        }

        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);

        ttl::extend_contract_ttl(&env, contract_id);

        true
    }
}
