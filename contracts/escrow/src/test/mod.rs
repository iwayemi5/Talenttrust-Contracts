#![cfg(test)]
#![allow(dead_code)]

use soroban_sdk::{testutils::Address as _, vec, Address, Env};

use crate::{Escrow, EscrowClient, ReleaseAuthorization};

// --- Submodules ---

mod client_migration;
mod emergency_controls;
mod pause_controls;
mod persistence;
mod release_authorization;
mod reputation;

// --- Shared constants ---

pub const MILESTONE_ONE: i128 = 200_0000000;
pub const MILESTONE_TWO: i128 = 400_0000000;
pub const MILESTONE_THREE: i128 = 600_0000000;

// --- Shared helpers ---

pub fn register_client(env: &Env) -> EscrowClient<'_> {
    let id = env.register(Escrow, ());
    let client = EscrowClient::new(env, &id);
    let admin = Address::generate(env);
    client.initialize(&admin);
    client
}

pub fn default_milestones(env: &Env) -> soroban_sdk::Vec<i128> {
    vec![env, MILESTONE_ONE, MILESTONE_TWO, MILESTONE_THREE]
}

pub fn total_milestone_amount() -> i128 {
    MILESTONE_ONE + MILESTONE_TWO + MILESTONE_THREE
}

#[allow(dead_code)]
pub fn total_milestones() -> i128 {
    total_milestone_amount()
}

#[allow(dead_code)]
pub fn generated_participants(env: &Env) -> (Address, Address) {
    (Address::generate(env), Address::generate(env))
}

/// Create a contract and return (client_addr, freelancer_addr, contract_id).
pub fn create_contract(env: &Env, client: &EscrowClient) -> (Address, Address, u32) {
    let client_addr = Address::generate(env);
    let freelancer_addr = Address::generate(env);
    let milestones = default_milestones(env);
    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );
    (client_addr, freelancer_addr, id)
}

/// Create a contract with an arbiter and return (client_addr, freelancer_addr, arbiter, contract_id).
pub fn create_contract_with_arbiter(
    env: &Env,
    client: &EscrowClient,
) -> (Address, Address, Address, u32) {
    let client_addr = Address::generate(env);
    let freelancer_addr = Address::generate(env);
    let arbiter_addr = Address::generate(env);
    let milestones = default_milestones(env);
    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &Some(arbiter_addr.clone()),
        &milestones,
        &ReleaseAuthorization::ClientAndArbiter,
    );
    (client_addr, freelancer_addr, arbiter_addr, id)
}

/// Create and fully complete a contract (all milestones released).
/// Caller is the client address for deposit and release operations.
pub fn complete_contract(env: &Env, client: &EscrowClient) -> (Address, Address, u32) {
    let (client_addr, freelancer_addr, id) = create_contract(env, client);
    assert!(client.deposit_funds(&id, &client_addr, &total_milestone_amount()));
    assert!(client.approve_milestone_release(&id, &client_addr, &0));
    assert!(client.release_milestone(&id, &client_addr, &0));
    assert!(client.approve_milestone_release(&id, &client_addr, &1));
    assert!(client.release_milestone(&id, &client_addr, &1));
    assert!(client.approve_milestone_release(&id, &client_addr, &2));
    assert!(client.release_milestone(&id, &client_addr, &2));
    (client_addr, freelancer_addr, id)
}

/// Assert that a `try_*` call returns the expected contract error.
///
/// Soroban `try_*` methods return:
///   `Result<Result<T, ConversionError>, Result<soroban_sdk::Error, InvokeError>>`
/// A contract-level `panic_with_error` surfaces as `Err(Ok(soroban_sdk::Error))`.
/// The `expected` argument can be any type convertible to `soroban_sdk::Error`,
/// including both `EscrowError` and the canonical `Error` from `types.rs`.
pub fn assert_contract_error<T, E: Into<soroban_sdk::Error> + core::fmt::Debug>(
    result: Result<T, Result<soroban_sdk::Error, soroban_sdk::InvokeError>>,
    expected: E,
) {
    match result {
        Err(Ok(e)) => {
            let expected_err: soroban_sdk::Error = expected.into();
            assert_eq!(e, expected_err, "contract error code mismatch");
        }
        _other => panic!(
            "expected contract error {:?}, got unexpected result variant",
            expected
        ),
    }
}

/// Helper: forcibly inject a contract status via env.as_contract.
pub fn set_escrow_status(env: &Env, escrow_addr: &Address, id: u32, status: ContractStatus) {
    use crate::{Contract as EscrowContract, DataKey};
    env.as_contract(escrow_addr, || {
        let key = DataKey::Contract(id);
        let _contract: EscrowContract = env.storage().persistent().get(&key).unwrap();
        let mut contract: crate::Contract = env.storage().persistent().get(&key).unwrap();
        contract.status = status;
        env.storage().persistent().set(&key, &contract);
    });
}
