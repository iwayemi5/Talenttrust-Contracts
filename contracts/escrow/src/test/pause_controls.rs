use crate::{Escrow, EscrowClient, EscrowError, ReleaseAuthorization};
use soroban_sdk::{testutils::Address as _, vec, Address, Env};

fn setup_initialized() -> (Env, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(Escrow, ());
    let client = EscrowClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    assert!(client.initialize(&admin));
    (env, contract_id, admin)
}

/// Create a funded contract with one milestone ready to release.
fn setup_funded_contract(env: &Env, client: &EscrowClient) -> (Address, Address, u32) {
    let client_addr = Address::generate(env);
    let freelancer_addr = Address::generate(env);
    let milestones = vec![env, 100_i128, 200_i128];
    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );
    client.deposit_funds(&id, &client_addr, &300_i128);
    (client_addr, freelancer_addr, id)
}

/// Create a completed contract ready for reputation issuance.
fn setup_completed_contract(env: &Env, client: &EscrowClient) -> (Address, Address, u32) {
    let (client_addr, freelancer_addr, id) = setup_funded_contract(env, client);
    client.approve_milestone_release(&id, &client_addr, &0);
    client.release_milestone(&id, &client_addr, &0);
    client.approve_milestone_release(&id, &client_addr, &1);
    client.release_milestone(&id, &client_addr, &1);
    (client_addr, freelancer_addr, id)
}

// ─── initialize ──────────────────────────────────────────────────────────────

#[test]
fn initialize_only_once_fails() {
    let (env, contract_id, admin) = setup_initialized();
    let client = EscrowClient::new(&env, &contract_id);
    super::assert_contract_error(
        client.try_initialize(&admin),
        EscrowError::AlreadyInitialized,
    );
}

// ─── pause / unpause ─────────────────────────────────────────────────────────

#[test]
fn pause_then_unpause_toggles_state() {
    let (env, contract_id, _admin) = setup_initialized();
    let client = EscrowClient::new(&env, &contract_id);

    assert!(!client.is_paused());
    assert!(client.pause());
    assert!(client.is_paused());
    assert!(client.unpause());
    assert!(!client.is_paused());
}

#[test]
fn pause_requires_initialization() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(Escrow, ());
    let client = EscrowClient::new(&env, &contract_id);
    super::assert_contract_error(client.try_pause(), EscrowError::NotInitialized);
}

// ─── create_contract blocked ─────────────────────────────────────────────────

#[test]
fn pause_blocks_create_contract() {
    let (env, contract_id, _admin) = setup_initialized();
    let client = EscrowClient::new(&env, &contract_id);
    client.pause();

    let a = Address::generate(&env);
    let b = Address::generate(&env);
    super::assert_contract_error(
        client.try_create_contract(
            &a,
            &b,
            &None,
            &vec![&env, 50_i128],
            &ReleaseAuthorization::ClientOnly,
        ),
        EscrowError::ContractPaused,
    );
}

// ─── deposit_funds blocked ───────────────────────────────────────────────────

#[test]
fn pause_blocks_deposit_funds() {
    let (env, contract_id, _admin) = setup_initialized();
    let client = EscrowClient::new(&env, &contract_id);
    let (_client_addr, _, id) = setup_funded_contract(&env, &client);
    client.pause();

    super::assert_contract_error(
        client.try_deposit_funds(&id, &client_addr, &50_i128),
        EscrowError::ContractPaused,
    );
}

// ─── release_milestone blocked ───────────────────────────────────────────────

#[test]
fn pause_blocks_release_milestone() {
    let (env, contract_id, _admin) = setup_initialized();
    let client = EscrowClient::new(&env, &contract_id);
    let (_client_addr, _, id) = setup_funded_contract(&env, &client);
    client.pause();

    super::assert_contract_error(
        client.try_release_milestone(&id, &client_addr, &0),
        EscrowError::ContractPaused,
    );
}

// ─── issue_reputation blocked ────────────────────────────────────────────────

#[test]
fn pause_blocks_issue_reputation() {
    let (env, contract_id, _admin) = setup_initialized();
    let client = EscrowClient::new(&env, &contract_id);
    let (client_addr, freelancer_addr, id) = setup_completed_contract(&env, &client);
    client.pause();

    super::assert_contract_error(
        client.try_issue_reputation(&id, &client_addr, &freelancer_addr, &5_i128),
        EscrowError::ContractPaused,
    );
}

// ─── cancel_contract blocked ─────────────────────────────────────────────────

#[test]
fn pause_blocks_cancel_contract() {
    let (env, contract_id, _admin) = setup_initialized();
    let client = EscrowClient::new(&env, &contract_id);
    let (client_addr, _, id) = setup_funded_contract(&env, &client);
    client.pause();

    super::assert_contract_error(
        client.try_cancel_contract(&id, &client_addr),
        EscrowError::ContractPaused,
    );
}

// ─── unpaused allows operations ──────────────────────────────────────────────

#[test]
fn unpause_restores_create_contract() {
    let (env, contract_id, _admin) = setup_initialized();
    let client = EscrowClient::new(&env, &contract_id);
    client.pause();
    client.unpause();

    let a = Address::generate(&env);
    let b = Address::generate(&env);
    let id = client.create_contract(
        &a,
        &b,
        &None,
        &vec![&env, 50_i128],
        &ReleaseAuthorization::ClientOnly,
    );
    assert_eq!(id, 1);
}
