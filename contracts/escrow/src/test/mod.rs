#![cfg(test)]
#![allow(dead_code)]

use soroban_sdk::{testutils::Address as _, vec, Address, Env, Vec};

use crate::{Contract, ContractStatus, Escrow, EscrowClient, EscrowError, Milestone, ReleaseAuthorization};

// --- Submodules ---

mod client_migration;
mod emergency_controls;
mod pause_controls;
mod persistence;
mod release_authorization;
mod reputation;
mod treasury_rotation_timelock;

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

// ---------------------------------------------------------------------------
// Helpers used by refund, release, deposit, and create_contract submodules
// ---------------------------------------------------------------------------

/// Returns `(Env, client_addr, freelancer_addr)` with all auths mocked.
pub fn setup() -> (Env, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);
    (env, client_addr, freelancer_addr)
}

/// Registers the Escrow contract and returns an `EscrowClient` bound to it.
pub fn create_client(env: &Env) -> EscrowClient<'_> {
    register_client(env)
}

/// Creates the default 3-milestone contract (200 / 400 / 600 stroops) and returns its id.
pub fn create_default_contract(
    env: &Env,
    client: &EscrowClient,
    client_addr: &Address,
    freelancer_addr: &Address,
) -> u32 {
    let milestones = vec![env, MILESTONE_ONE, MILESTONE_TWO, MILESTONE_THREE];
    client.create_contract(
        client_addr,
        freelancer_addr,
        &None,
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    )
}

/// Asserts `funded_amount`, `released_amount`, `refunded_amount`, and `status` on a `Contract`.
pub fn assert_contract_state(
    contract: Contract,
    expected_status: ContractStatus,
    expected_funded: i128,
    expected_released: i128,
    expected_refunded: i128,
) {
    assert_eq!(contract.status, expected_status, "status mismatch");
    assert_eq!(
        contract.funded_amount, expected_funded,
        "funded_amount mismatch"
    );
    assert_eq!(
        contract.released_amount, expected_released,
        "released_amount mismatch"
    );
    assert_eq!(
        contract.refunded_amount, expected_refunded,
        "refunded_amount mismatch"
    );
}

/// Asserts `released` and `refunded` flags on the milestone at `index`.
pub fn assert_milestone_flags(
    milestones: Vec<Milestone>,
    index: u32,
    expected_released: bool,
    expected_refunded: bool,
) {
    let m = milestones.get(index).expect("milestone index out of bounds");
    assert_eq!(m.released, expected_released, "milestone.released mismatch at index {index}");
    assert_eq!(m.refunded, expected_refunded, "milestone.refunded mismatch at index {index}");
}

/// Alias used by `hello.rs`.
pub fn setup_env() -> Env {
    let env = Env::default();
    env.mock_all_auths();
    env
}

/// Alias used by `hello.rs`.
pub fn register_escrow(env: &Env) -> EscrowClient<'_> {
    register_client(env)
}
