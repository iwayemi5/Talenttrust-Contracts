use crate::{DataKey, Escrow, EscrowArgs, EscrowClient};
use soroban_sdk::{contractimpl, symbol_short, Address, Env, Symbol};

#[allow(dead_code)]
impl super::Escrow {
    pub(crate) fn set_protocol_fee_bps_impl(env: &Env, new_bps: u32) -> bool {
        Self::require_initialized(env);

        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| env.panic_with_error(crate::Error::NotInitialized));
        admin.require_auth();

        let old_bps: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::ProtocolFeeBps)
            .unwrap_or(0u32);
        env.storage()
            .persistent()
            .set(&DataKey::ProtocolFeeBps, &new_bps);

        env.events().publish(
            (Symbol::new(env, "protocol_fee_bps"),),
            (old_bps, new_bps, admin.clone(), env.ledger().timestamp()),
        );
        true
    }

    pub(crate) fn propose_governance_admin_impl(env: &Env, proposed: Address) -> bool {
        Self::require_initialized(env);

        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| env.panic_with_error(crate::Error::NotInitialized));
        admin.require_auth();

        if proposed == admin {
            env.panic_with_error(crate::Error::CannotProposeSelf);
        }

        env.storage()
            .persistent()
            .set(&DataKey::PendingAdmin, &proposed);

        env.events().publish(
            (symbol_short!("admin"), Symbol::new(env, "proposed")),
            (admin, proposed.clone(), env.ledger().timestamp()),
        );
        true
    }

    pub(crate) fn accept_governance_admin_impl(env: &Env) -> bool {
        Self::require_initialized(env);

        let pending: Option<Address> = env.storage().persistent().get(&DataKey::PendingAdmin);
        if pending.is_none() {
            env.panic_with_error(crate::Error::InvalidState);
        }
        let pending_admin = pending.unwrap();
        pending_admin.require_auth();

        let old_admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| env.panic_with_error(crate::Error::NotInitialized));

        env.storage()
            .persistent()
            .set(&DataKey::Admin, &pending_admin);
        env.storage().persistent().remove(&DataKey::PendingAdmin);

        env.events().publish(
            (symbol_short!("admin"), Symbol::new(env, "accepted")),
            (old_admin, pending_admin.clone(), env.ledger().timestamp()),
        );
        true
    }
}
