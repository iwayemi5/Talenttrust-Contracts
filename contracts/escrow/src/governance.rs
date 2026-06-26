use crate::{DataKey, EscrowError};
use crate::ttl::ADMIN_ROTATION_MIN_DELAY_LEDGERS;
use crate::types::PendingAdminProposal;
use soroban_sdk::{symbol_short, Address, Env, Symbol};

/// Governance-related privileged operations and audit events.
///
/// Functions here are plain inherent helpers called by thin wrappers in
/// `lib.rs` that are part of the main `#[contractimpl]` block. This keeps
/// the generated `EscrowClient` free of duplicate-symbol conflicts.
#[allow(dead_code)]
impl super::Escrow {
    /// Set the protocol fee (basis points). Emits an event with
    /// `(old_bps, new_bps, admin, timestamp)` under topic `protocol_fee_bps`.
    pub fn set_protocol_fee_bps(env: Env, new_bps: u32) -> bool {
        if !env
            .storage()
            .persistent()
            .get::<_, bool>(&crate::DataKey::Initialized)
            .unwrap_or(false)
        {
            env.panic_with_error(EscrowError::NotInitialized);
        }

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

    /// Internal: propose a new admin with a timelock.
    pub(crate) fn propose_governance_admin_impl(env: Env, proposed: Address) -> bool {
        if !env
            .storage()
            .persistent()
            .get::<_, bool>(&crate::DataKey::Initialized)
            .unwrap_or(false)
        {
            env.panic_with_error(EscrowError::NotInitialized);
        }

        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| env.panic_with_error(crate::Error::NotInitialized));
        admin.require_auth();

        env.storage().persistent().set(
            &DataKey::PendingAdmin,
            &PendingAdminProposal {
                proposed: proposed.clone(),
                proposed_at_ledger: env.ledger().sequence(),
            },
        );

        env.events().publish(
            (symbol_short!("admin"), Symbol::new(env, "proposed")),
            (admin, proposed.clone(), env.ledger().timestamp()),
        );
        true
    }

    /// Internal: accept a pending admin proposal, enforcing the timelock.
    pub(crate) fn accept_governance_admin_impl(env: Env) -> bool {
        if !env
            .storage()
            .persistent()
            .get::<_, bool>(&crate::DataKey::Initialized)
            .unwrap_or(false)
        {
            env.panic_with_error(EscrowError::NotInitialized);
        }

        let pending: Option<PendingAdminProposal> =
            env.storage().persistent().get(&DataKey::PendingAdmin);
        if pending.is_none() {
            env.panic_with_error(crate::Error::InvalidState);
        }
        let proposal = pending.unwrap();

        // Enforce treasury rotation timelock: acceptance is only allowed after
        // ADMIN_ROTATION_MIN_DELAY_LEDGERS have elapsed since the proposal.
        let elapsed = env
            .ledger()
            .sequence()
            .saturating_sub(proposal.proposed_at_ledger);
        if elapsed < ADMIN_ROTATION_MIN_DELAY_LEDGERS {
            env.panic_with_error(EscrowError::TimelockNotElapsed);
        }

        let pending_admin = proposal.proposed;
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

    /// Internal: return the currently pending admin address, if any.
    pub(crate) fn get_pending_governance_admin_impl(env: Env) -> Option<Address> {
        let proposal: Option<PendingAdminProposal> =
            env.storage().persistent().get(&DataKey::PendingAdmin);
        proposal.map(|p| p.proposed)
    }

    /// Internal: return the current admin address.
    pub(crate) fn get_governance_admin_impl(env: Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::Admin)
    }
}
