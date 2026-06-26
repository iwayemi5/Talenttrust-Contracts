use soroban_sdk::{contracterror, contracttype, Address, String, Vec};

/// Pending treasury/admin rotation proposal, stored under [`DataKey::PendingAdmin`].
///
/// Captures the proposed new admin and the ledger at which the proposal was
/// made so that `accept_governance_admin` can enforce `ADMIN_ROTATION_MIN_DELAY_LEDGERS`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingAdminProposal {
    pub proposed: Address,
    pub proposed_at_ledger: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    // Admin / pause / emergency
    Initialized,
    Admin,
    Paused,
    Emergency,
    // Contract storage
    Contract(u32),
    NextContractId,
    MilestoneApprovals(u32, u32),
    // Reputation
    ReputationIssued(u32),
    PendingReputationCredits(Address),
    Reputation(Address),
    // Client migration
    PendingClientMigration(u32),
    // Protocol / governance
    GovernanceAdmin,
    PendingGovernanceAdmin,
    ProtocolParameters,
    ProtocolFeeBps,
    // Two-step admin transfer: pending admin stored here while proposal awaits acceptance
    PendingAdmin,
    AccumulatedProtocolFees,
    GovernedParameters,
    ReadinessChecklist,
    // Finalization
    Finalization(u32),
    // SAC settlement token — bound once at admin setup; deposit_funds and
    // release_milestone read this to wire on-chain custody through the
    // configured Stellar Asset Contract.
    SettlementToken,
}

/// Canonical contract error type for all entrypoint-facing errors.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    IndexOutOfBounds = 3,
    AlreadyReleased = 4,
    EmptyRefundRequest = 6,
    DuplicateMilestoneInRefund = 7,
    AlreadyRefunded = 8,
    InsufficientFunds = 9,
    ContractNotFound = 10,
    UnauthorizedRole = 11,
    MissingArbiter = 12,
    InvalidArbiter = 13,
    InvalidParticipants = 14,
    AmountMustBePositive = 15,
    InvalidState = 16,
    MilestoneAlreadyReleased = 17,
    AlreadyApproved = 18,
    InsufficientApprovals = 20,
    FreelancerMismatch = 21,
    InvalidRating = 22,
    ReputationAlreadyIssued = 23,
    EmptyMilestones = 25,
    InvalidMilestoneAmount = 26,
    ContractIdCollision = 27,
    ContractIdOverflow = 28,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Contract {
    pub client: Address,
    pub freelancer: Address,
    pub arbiter: Option<Address>,
    pub status: ContractStatus,
    pub funded_amount: i128,
    pub released_amount: i128,
    pub refunded_amount: i128,
    pub release_authorization: ReleaseAuthorization,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneApprovals {
    pub client_approved: bool,
    pub freelancer_approved: bool,
    pub arbiter_approved: bool,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReleaseAuthorization {
    /// Only client can approve.
    ClientOnly = 0,
    /// Either client or arbiter can approve.
    ClientAndArbiter = 1,
    /// Only arbiter can approve.
    ArbiterOnly = 2,
    /// Both client and freelancer must approve; only either of them may release
    /// after both approvals are present.
    MultiSig = 3,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    // Admin / pause / emergency
    Initialized,
    Admin,
    Paused,
    Emergency,
    // Contract storage
    Contract(u32),
    NextContractId,
    MilestoneReleased(u32, u32),
    MilestoneApprovals(u32, u32),
    // Participant indexer (append-only contract id lists)
    ClientContracts(Address),
    FreelancerContracts(Address),
    // Reputation
    ReputationIssued(u32),
    PendingReputationCredits(Address),
    Reputation(Address),
    // Client migration
    PendingClientMigration(u32),
    // Protocol / governance
    GovernanceAdmin,
    PendingGovernanceAdmin,
    ProtocolParameters,
    ProtocolFeeBps,
    // Two-step admin transfer: pending admin stored here while proposal awaits acceptance
    PendingAdmin,
    AccumulatedProtocolFees,
    GovernedParameters,
    ReadinessChecklist,
    // Finalization
    Finalization(u32),
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContractStatus {
    Created = 0,
    Accepted = 1,
    Funded = 2,
    Completed = 3,
    Disputed = 4,
    Cancelled = 5,
    Refunded = 6,
    PartiallyFunded = 7,
}

/// Outcome selected by the arbiter when resolving a dispute.
///
/// `Release` marks the dispute as resolved in favour of the freelancer,
/// `Refund` in favour of the client, and `Cancel` simply terminates the
/// contract without moving funds. Splits are issued through the dedicated
/// [`crate::Escrow::resolve_dispute_split`] entry point because the
/// Soroban `contracttype` macro only accepts unit variants on enums.
#[contracttype]
#[repr(u32)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DisputeResolution {
    /// Release every remaining unreleased milestone to the freelancer.
    Release = 0,
    /// Refund every remaining unreleased milestone to the client.
    Refund = 1,
    /// Cancel the contract without moving funds.
    Cancel = 2,
}

/// Numeric code that the event-emitter publishes for the resolution
/// variant. Kept here so it ships in lockstep with `DisputeResolution`
/// and the [`crate::Escrow::resolve_dispute_split`] dedicated flow.
pub const DISPUTE_RESOLUTION_RELEASE: u32 = 0;
pub const DISPUTE_RESOLUTION_REFUND: u32 = 1;
pub const DISPUTE_RESOLUTION_CANCEL: u32 = 2;
pub const DISPUTE_RESOLUTION_SPLIT: u32 = 3;

/// Arbiter-driven split of the available escrow balance. Carried as a
/// separate `contracttype` struct so that the `DisputeResolution` enum can
/// stay unit-only (which is what `#[contracttype]` supports).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisputeSplit {
    /// Amount (in stroops) that goes to the client.
    pub client_amount: i128,
    /// Amount (in stroops) that goes to the freelancer.
    pub freelancer_amount: i128,
}

/// Metadata recorded when a dispute is raised on a contract.
///
/// Persisted under [`DataKey::Dispute`] keyed by contract id.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisputeMetadata {
    /// Address that raised the dispute (must be client or freelancer).
    pub raised_by: Address,
    /// Cryptographic hash of the off-chain reason / evidence.
    pub reason_hash: BytesN<32>,
    /// Ledger timestamp at which the dispute was raised.
    pub raised_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Milestone {
    pub amount: i128,
    pub funded_amount: i128,
    pub released: bool,
    pub refunded: bool,
    pub work_evidence: Option<String>,
    pub refunded_amount: i128,
    pub deadline: Option<u64>,
}

/// Protocol version shipped with this build.
pub const MAINNET_PROTOCOL_VERSION: u32 = 1;

/// Maximum total escrow value per contract in stroops (1 XLM = 10_000_000 stroops).
/// Hard cap: 100_000_000 XLM.
pub const MAINNET_MAX_TOTAL_ESCROW_PER_CONTRACT_STROOPS: i128 = 1_000_000_000_000_000;

/// Readiness checklist stored under [`DataKey::ReadinessChecklist`].
///
/// Returned by `get_mainnet_readiness_info`. All boolean fields default to
/// `false` on a fresh deployment; they flip to `true` as the operator
/// progresses through the initialization sequence.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadinessChecklist {
    /// `true` after `initialize(admin)` has been called successfully.
    pub initialized: bool,
    /// `true` after `set_governed_params` has been called successfully.
    pub governed_params_set: bool,
    /// `true` after an emergency control operation has been invoked.
    pub emergency_controls_enabled: bool,
    /// `true` when `MAINNET_MAX_TOTAL_ESCROW_PER_CONTRACT_STROOPS > 0`.
    /// Derived at runtime from the compile-time constant; never written to storage.
    pub caps_set: bool,
    /// Protocol version compiled into this build.
    pub protocol_version: u32,
    /// Compile-time maximum escrow total per contract, in stroops.
    pub max_escrow_total_stroops: i128,
}

impl Default for ReadinessChecklist {
    fn default() -> Self {
        ReadinessChecklist {
            initialized: false,
            governed_params_set: false,
            emergency_controls_enabled: false,
            caps_set: MAINNET_MAX_TOTAL_ESCROW_PER_CONTRACT_STROOPS > 0,
            protocol_version: MAINNET_PROTOCOL_VERSION,
            max_escrow_total_stroops: MAINNET_MAX_TOTAL_ESCROW_PER_CONTRACT_STROOPS,
        }
    }
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GovernedParameters {
    pub protocol_fee_bps: u32,
    pub max_escrow_total_stroops: i128,
}

// ─── Indexer summary types ────────────────────────────────────────────────────

/// Current schema version for [`ContractSummary`].
///
/// Bumped when a breaking change is made to the summarised shape.
/// Downstream indexers MUST branch on this field to decode the
/// rest of the struct correctly.
///
/// # Current value
/// `1`
///
/// # Versioning policy
/// See `docs/escrow/indexer-schema.md`.
#[allow(dead_code)]
pub const CONTRACT_SUMMARY_SCHEMA_VERSION: u32 = 1;

/// Per-milestone summary embedded in [`ContractSummary`].
///
/// A lightweight projection of the on-chain [`Milestone`] that omits
/// internal accounting fields (`funded_amount`, `refunded_amount`,
/// `work_evidence`) that are not relevant to off-chain indexers.
///
/// # Fields
/// * `index` – 0-based position in the milestones vector.
/// * `amount` – Original amount specified at contract creation (stroops).
/// * `released` – `true` after [`release_milestone`] succeeds.
/// * `refunded` – `true` after [`refund_unreleased_milestones`] includes this
///   index.
///
/// [`release_milestone`]: crate::Escrow::release_milestone
/// [`refund_unreleased_milestones`]: crate::Escrow::refund_unreleased_milestones
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneSummary {
    /// 0-based milestone index.
    pub index: u32,
    /// Original milestone amount in stroops (set at creation).
    pub amount: i128,
    /// Whether this milestone has been released.
    pub released: bool,
    /// Whether this milestone has been refunded.
    pub refunded: bool,
}

/// Denormalised, versioned snapshot of an escrow contract for off-chain
/// indexers.
///
/// Produced during [`finalize_contract`] and stored as part of the
/// finalization record.
///
/// # Field provenance
///
/// | Classification | Fields |
/// |---|---|
/// | Copied verbatim from [`Contract`] | `client`, `freelancer`, `arbiter`, `status`, `funded_amount`, `released_amount` |
/// | Per-milestone projection from stored [`Milestone`] records | `milestones` |
/// | Derived at finalisation | `total_amount`, `refundable_balance`, `released_milestone_count` |
/// | Set to [`CONTRACT_SUMMARY_SCHEMA_VERSION`] | `schema_version` |
/// | Hardcoded (see caveat) | `reputation_issued` |
///
/// # Versioning
/// See `docs/escrow/indexer-schema.md`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractSummary {
    /// Schema version used to produce this snapshot.
    /// Indexers MUST check this before decoding.
    pub schema_version: u32,
    /// Client address (copied from [`Contract::client`]).
    pub client: Address,
    /// Freelancer address (copied from [`Contract::freelancer`]).
    pub freelancer: Address,
    /// Optional arbiter address (copied from [`Contract::arbiter`]).
    pub arbiter: Option<Address>,
    /// Contract status at finalisation time (copied from [`Contract::status`]).
    pub status: ContractStatus,
    /// **Hardcoded to `false`** – not yet wired to the on-chain
    /// `DataKey::ReputationIssued` flag.
    ///
    /// # Caveat
    /// This field does NOT reflect the actual reputation issuance state.
    /// See `docs/escrow/indexer-schema.md` for details.
    pub reputation_issued: bool,
    /// Sum of every milestone's `amount` field (derived at finalisation).
    pub total_amount: i128,
    /// Total amount deposited by the client (copied from
    /// [`Contract::funded_amount`]).
    pub funded_amount: i128,
    /// Total amount released to the freelancer (copied from
    /// [`Contract::released_amount`]).
    pub released_amount: i128,
    /// Remaining escrow balance that can be refunded (derived).
    ///
    /// Computed as:
    /// ```text
    /// refundable_balance = (funded_amount - released_amount) - refunded_amount
    /// ```
    pub refundable_balance: i128,
    /// Number of milestones with `released == true` (derived).
    pub released_milestone_count: u32,
    /// Per-milestone summary entries.
    pub milestones: Vec<MilestoneSummary>,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DepositMode {
    ExactTotal = 0,
    Incremental = 1,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct Reputation {
    pub completed_contracts: i128,
    pub total_rating: i128,
    pub last_rating: i128,
}
