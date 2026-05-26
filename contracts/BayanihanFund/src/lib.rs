#![no_std]
use soroban_sdk::{
    contract, contractimpl, contracttype, token, Address, Bytes, Env, Symbol, symbol_short,
    String,
};

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    Token,
    TotalDonated,
    TotalDisbursed,
    Relief(u64),        // relief request id → ReliefRequest
    Beneficiary(Address, u64), // double-key: beneficiary × request_id → claimed bool
    NextId,
}

// ── Data Types ────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, PartialEq)]
pub enum ReliefStatus {
    Open,
    Approved,
    Disbursed,
    Cancelled,
}

#[contracttype]
#[derive(Clone)]
pub struct ReliefRequest {
    pub id: u64,
    pub requester: Address,     // barangay LGU or NGO wallet
    pub description: String,    // e.g. "Typhoon Egay relief - Batangas, 200 families"
    pub amount_requested: i128, // total amount needed
    pub amount_per_family: i128,// USDC per beneficiary
    pub beneficiary_count: u32, // number of registered beneficiaries
    pub claimed_count: u32,     // increments as each beneficiary claims
    pub status: ReliefStatus,
    pub created_at: u64,
    pub approved_at: u64,
}

// ── Events ────────────────────────────────────────────────────────────────────
const DONATED: Symbol = symbol_short!("DONATED");
const REQUESTED: Symbol = symbol_short!("REQSTD");
const APPROVED: Symbol = symbol_short!("APPROVED");
const CLAIMED: Symbol = symbol_short!("CLAIMED");

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct BayanihanFundContract;

#[contractimpl]
impl BayanihanFundContract {
    /// Initialize: admin is typically an NGO multisig or DAO.
    pub fn initialize(env: Env, admin: Address, token: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("already initialized");
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::NextId, &0u64);
        env.storage().instance().set(&DataKey::TotalDonated, &0i128);
        env.storage().instance().set(&DataKey::TotalDisbursed, &0i128);
    }

    /// Anyone can donate USDC to the fund. Full on-chain transparency.
    pub fn donate(env: Env, donor: Address, amount: i128) {
        donor.require_auth();
        assert!(amount > 0, "donation must be positive");

        let token: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&donor, &env.current_contract_address(), &amount);

        let total: i128 = env.storage().instance().get(&DataKey::TotalDonated).unwrap();
        env.storage().instance().set(&DataKey::TotalDonated, &(total + amount));

        env.events().publish((DONATED, donor), amount);
    }

    /// Barangay or NGO submits a relief request for a disaster event.
    pub fn submit_request(
        env: Env,
        requester: Address,
        description: String,
        amount_per_family: i128,
        beneficiary_count: u32,
    ) -> u64 {
        requester.require_auth();
        assert!(beneficiary_count > 0, "at least 1 beneficiary");
        assert!(amount_per_family > 0, "amount per family must be positive");

        let id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap();
        env.storage().instance().set(&DataKey::NextId, &(id + 1));

        let total_needed = amount_per_family * beneficiary_count as i128;

        let req = ReliefRequest {
            id,
            requester: requester.clone(),
            description,
            amount_requested: total_needed,
            amount_per_family,
            beneficiary_count,
            claimed_count: 0,
            status: ReliefStatus::Open,
            created_at: env.ledger().timestamp(),
            approved_at: 0,
        };
        env.storage().persistent().set(&DataKey::Relief(id), &req);
        env.events().publish((REQUESTED, requester), (id, total_needed));
        id
    }

    /// Admin approves the request after verifying disaster and beneficiary list off-chain.
    /// In production this would be a multi-sig or on-chain vote.
    pub fn approve_request(env: Env, relief_id: u64) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let mut req: ReliefRequest = env
            .storage()
            .persistent()
            .get(&DataKey::Relief(relief_id))
            .expect("not found");

        assert!(req.status == ReliefStatus::Open, "request not open");

        // Check fund has enough balance
        let token: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let token_client = token::Client::new(&env, &token);
        let balance = token_client.balance(&env.current_contract_address());
        assert!(balance >= req.amount_requested, "insufficient fund balance");

        req.status = ReliefStatus::Approved;
        req.approved_at = env.ledger().timestamp();
        env.storage().persistent().set(&DataKey::Relief(relief_id), &req);

        env.events().publish((APPROVED,), (relief_id, req.amount_requested));
    }

    /// Each beneficiary claims their individual relief payment.
    /// This is the MVP: permissionless individual claim after approval.
    pub fn claim_relief(env: Env, relief_id: u64, beneficiary: Address) {
        beneficiary.require_auth();

        // Prevent double-claim
        let claimed_key = DataKey::Beneficiary(beneficiary.clone(), relief_id);
        if env.storage().persistent().has(&claimed_key) {
            panic!("already claimed");
        }

        let mut req: ReliefRequest = env
            .storage()
            .persistent()
            .get(&DataKey::Relief(relief_id))
            .expect("relief not found");

        assert!(req.status == ReliefStatus::Approved, "not approved");
        assert!(
            req.claimed_count < req.beneficiary_count,
            "all slots claimed"
        );

        // Mark claimed (re-entrancy guard)
        env.storage().persistent().set(&claimed_key, &true);
        req.claimed_count += 1;

        // If all claimed, mark disbursed
        if req.claimed_count == req.beneficiary_count {
            req.status = ReliefStatus::Disbursed;
        }
        env.storage().persistent().set(&DataKey::Relief(relief_id), &req);

        // Transfer to beneficiary
        let token: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let token_client = token::Client::new(&env, &token);
        token_client.transfer(
            &env.current_contract_address(),
            &beneficiary,
            &req.amount_per_family,
        );

        let disbursed: i128 = env.storage().instance().get(&DataKey::TotalDisbursed).unwrap();
        env.storage()
            .instance()
            .set(&DataKey::TotalDisbursed, &(disbursed + req.amount_per_family));

        env.events().publish((CLAIMED, beneficiary), (relief_id, req.amount_per_family));
    }

    /// Read relief request details.
    pub fn get_request(env: Env, relief_id: u64) -> ReliefRequest {
        env.storage()
            .persistent()
            .get(&DataKey::Relief(relief_id))
            .expect("not found")
    }

    /// Public fund dashboard: total donated vs disbursed.
    pub fn fund_stats(env: Env) -> (i128, i128) {
        let donated: i128 = env.storage().instance().get(&DataKey::TotalDonated).unwrap_or(0);
        let disbursed: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalDisbursed)
            .unwrap_or(0);
        (donated, disbursed)
    }

    /// Admin cancels a request (e.g. fraudulent claim discovered).
    pub fn cancel_request(env: Env, relief_id: u64) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let mut req: ReliefRequest = env
            .storage()
            .persistent()
            .get(&DataKey::Relief(relief_id))
            .expect("not found");

        assert!(req.status != ReliefStatus::Disbursed, "already fully disbursed");
        req.status = ReliefStatus::Cancelled;
        env.storage().persistent().set(&DataKey::Relief(relief_id), &req);
    }
}
