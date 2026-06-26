//! Property tests for resolution_payouts invariant

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use soroban_sdk::Address;

    // Helper to create a dummy contract with given funded, released, refunded amounts
    fn dummy_contract(available: i128) -> Contract {
        // For simplicity, create a contract where funded_amount = available,
        // and released and refunded are zero. This satisfies the calculation
        // of `available` inside `resolution_payouts`.
        Contract {
            client: Address::generate(&Env::default()),
            freelancer: Address::generate(&Env::default()),
            arbiter: Some(Address::generate(&Env::default())),
            funded_amount: available,
            released_amount: 0,
            refunded_amount: 0,
            total_deposited: available,
            status: ContractStatus::Funded,
            // other fields defaulted/zeroed as needed
            ..Default::default()
        }
    }

    proptest! {
        #[test]
        fn split_preserves_available_balance(
            // generate a non‑negative total available amount
            available in 0i128..=i128::MAX,
            // generate client and freelancer amounts that sum to `available`
            client_amount in 0i128..=i128::MAX,
        ) {
            // Ensure the client amount does not exceed the available balance
            prop_assume!(client_amount <= available);
            let freelancer_amount = available - client_amount;

            let contract = dummy_contract(available);
            let resolution = DisputeResolution::Split(client_amount, freelancer_amount);

            let (client_payout, freelancer_payout) =
                resolution_payouts(&contract, &resolution).expect("valid split should succeed");

            // Invariant: client + freelancer == available
            prop_assert_eq!(client_payout + freelancer_payout, available);
        }
    }

    proptest! {
        #[test]
        fn split_rejects_invalid_total(
            available in 0i128..=i128::MAX,
            client_amount in 0i128..=i128::MAX,
            extra in 1i128..=1000,
        ) {
            // Create a total that does NOT equal `available`
            let total = client_amount + extra;
            prop_assume!(total != available);
            let freelancer_amount = total - client_amount;

            // Ensure amounts are non‑negative
            prop_assume!(freelancer_amount >= 0);

            let contract = dummy_contract(available);
            let resolution = DisputeResolution::Split(client_amount, freelancer_amount);

            let result = resolution_payouts(&contract, &resolution);
            prop_assert!(result.is_err());
        }
    }
}
