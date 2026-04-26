#![cfg(test)]

use crate::{ContractStatus, Escrow, EscrowClient, EscrowError};
use soroban_sdk::{testutils::Address as _, testutils::Ledger as _, vec, Address, Env};

fn setup() -> (Env, EscrowClient, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(Escrow, ());
    let client = EscrowClient::new(&env, &contract_id);
    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);
    (env, client, client_addr, freelancer_addr)
}

#[test]
fn test_approval_expiry_success() {
    let (env, client, client_addr, freelancer_addr) = setup();

    let milestones = vec![&env, 100_0000000_i128];
    let expiry_window = 3600; // 1 hour

    let contract_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones,
        &None,
        &None,
        &Some(expiry_window),
    );

    client.deposit_funds(&contract_id, &100_0000000_i128);

    // Approve milestone
    client.approve_milestone(&contract_id, &0);

    // Release within window (current time)
    assert!(client.release_milestone(&contract_id, &0));

    let contract = client.get_contract(&contract_id);
    assert_eq!(contract.released_amount, 100_0000000_i128);
}

#[test]
fn test_approval_expiry_failure() {
    let (env, client, client_addr, freelancer_addr) = setup();

    let milestones = vec![&env, 100_0000000_i128];
    let expiry_window = 3600; // 1 hour

    let contract_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones,
        &None,
        &None,
        &Some(expiry_window),
    );

    client.deposit_funds(&contract_id, &100_0000000_i128);

    // Approve milestone at T=0
    env.ledger().set_timestamp(1000);
    client.approve_milestone(&contract_id, &0);

    // Fast forward past expiry (1000 + 3600 + 1)
    env.ledger().set_timestamp(1000 + 3600 + 1);

    // Release should fail
    let result = client.try_release_milestone(&contract_id, &0);
    assert_eq!(result, Err(Ok(EscrowError::ApprovalExpired)));
}

#[test]
fn test_reapproval_resets_expiry() {
    let (env, client, client_addr, freelancer_addr) = setup();

    let milestones = vec![&env, 100_0000000_i128];
    let expiry_window = 3600; // 1 hour

    let contract_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones,
        &None,
        &None,
        &Some(expiry_window),
    );

    client.deposit_funds(&contract_id, &100_0000000_i128);

    // First approval at T=1000
    env.ledger().set_timestamp(1000);
    client.approve_milestone(&contract_id, &0);

    // Move to T=4000 (past first expiry)
    env.ledger().set_timestamp(4000);
    let result = client.try_release_milestone(&contract_id, &0);
    assert_eq!(result, Err(Ok(EscrowError::ApprovalExpired)));

    // Re-approve at T=4000
    client.approve_milestone(&contract_id, &0);

    // Move to T=7000 (within new expiry window: 4000 to 7600)
    env.ledger().set_timestamp(7000);
    assert!(client.release_milestone(&contract_id, &0));
}

#[test]
fn test_no_expiry_if_not_set() {
    let (env, client, client_addr, freelancer_addr) = setup();

    let milestones = vec![&env, 100_0000000_i128];

    let contract_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones,
        &None,
        &None,
        &None, // No expiry window
    );

    client.deposit_funds(&contract_id, &100_0000000_i128);

    // Approve at T=1000
    env.ledger().set_timestamp(1000);
    client.approve_milestone(&contract_id, &0);

    // Move very far in the future
    env.ledger().set_timestamp(1000 + 1_000_000);

    // Should still work
    assert!(client.release_milestone(&contract_id, &0));
}
