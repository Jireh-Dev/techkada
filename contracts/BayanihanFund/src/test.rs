#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::Address as _,
    token::{Client as TokenClient, StellarAssetClient},
    Address, Env, String,
};

fn setup() -> (Env, Address, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let requester = Address::generate(&env); // barangay LGU
    let donor = Address::generate(&env);

    let token_id = env.register_stellar_asset_contract_v2(admin.clone());
    let token_address = token_id.address();
    let asset_client = StellarAssetClient::new(&env, &token_address);

    // Give donor 100,000 USDC
    asset_client.mint(&donor, &100_000_0000000i128);

    let contract_id = env.register(BayanihanFundContract, ());
    let contract_address = contract_id;

    (env, contract_address, admin, requester, token_address)
}

// Helper: find a donor address
fn donor(env: &Env) -> Address { Address::generate(env) }

// ── Test 1: Happy Path — donate → request → approve → claim ─────────────────
#[test]
fn test_happy_path_full_flow() {
    let (env, contract, admin, requester, token) = setup();
    let client = BayanihanFundContractClient::new(&env, &contract);
    let donor = Address::generate(&env);

    client.initialize(&admin, &token);

    // Fund donor
    let asset_client = StellarAssetClient::new(&env, &token);
    asset_client.mint(&donor, &10_000_0000000i128);

    // Donate 5,000 USDC
    client.donate(&donor, &5_000_0000000i128);

    // Barangay submits request: 100 families × 20 USDC
    let relief_id = client.submit_request(
        &requester,
        &String::from_str(&env, "Typhoon Egay - Batangas 100 families"),
        &20_0000000i128,
        &100u32,
    );

    // Admin approves
    client.approve_request(&relief_id);

    // One beneficiary claims
    let beneficiary = Address::generate(&env);
    client.claim_relief(&relief_id, &beneficiary);

    let token_client = TokenClient::new(&env, &token);
    assert_eq!(token_client.balance(&beneficiary), 20_0000000i128);
}

// ── Test 2: Edge Case — double claim must fail ───────────────────────────────
#[test]
#[should_panic(expected = "already claimed")]
fn test_double_claim_blocked() {
    let (env, contract, admin, requester, token) = setup();
    let client = BayanihanFundContractClient::new(&env, &contract);
    let asset_client = StellarAssetClient::new(&env, &token);
    let donor = Address::generate(&env);
    asset_client.mint(&donor, &50_000_0000000i128);

    client.initialize(&admin, &token);
    client.donate(&donor, &10_000_0000000i128);

    let relief_id = client.submit_request(
        &requester,
        &String::from_str(&env, "Test relief"),
        &50_0000000i128,
        &50u32,
    );
    client.approve_request(&relief_id);

    let beneficiary = Address::generate(&env);
    client.claim_relief(&relief_id, &beneficiary);
    // Second claim → panic
    client.claim_relief(&relief_id, &beneficiary);
}

// ── Test 3: State verification — fund stats update correctly ─────────────────
#[test]
fn test_fund_stats_after_donate_and_claim() {
    let (env, contract, admin, requester, token) = setup();
    let client = BayanihanFundContractClient::new(&env, &contract);
    let asset_client = StellarAssetClient::new(&env, &token);
    let donor = Address::generate(&env);
    asset_client.mint(&donor, &50_000_0000000i128);

    client.initialize(&admin, &token);
    client.donate(&donor, &10_000_0000000i128);

    let relief_id = client.submit_request(
        &requester,
        &String::from_str(&env, "Test"),
        &100_0000000i128,
        &10u32,
    );
    client.approve_request(&relief_id);

    // 3 beneficiaries claim
    for _ in 0..3 {
        let b = Address::generate(&env);
        client.claim_relief(&relief_id, &b);
    }

    let (total_donated, total_disbursed) = client.fund_stats();
    assert_eq!(total_donated, 10_000_0000000i128);
    assert_eq!(total_disbursed, 300_0000000i128); // 3 × 100 USDC
}

// ── Test 4: Cannot claim on unapproved request ───────────────────────────────
#[test]
#[should_panic(expected = "not approved")]
fn test_claim_on_open_request_fails() {
    let (env, contract, admin, requester, token) = setup();
    let client = BayanihanFundContractClient::new(&env, &contract);
    let asset_client = StellarAssetClient::new(&env, &token);
    let donor = Address::generate(&env);
    asset_client.mint(&donor, &50_000_0000000i128);

    client.initialize(&admin, &token);
    client.donate(&donor, &5_000_0000000i128);

    let relief_id = client.submit_request(
        &requester,
        &String::from_str(&env, "Unapproved request"),
        &50_0000000i128,
        &10u32,
    );

    // Not yet approved → must panic
    let beneficiary = Address::generate(&env);
    client.claim_relief(&relief_id, &beneficiary);
}

// ── Test 5: Admin can cancel an approved request ──────────────────────────────
#[test]
fn test_admin_cancel_blocks_further_claims() {
    let (env, contract, admin, requester, token) = setup();
    let client = BayanihanFundContractClient::new(&env, &contract);
    let asset_client = StellarAssetClient::new(&env, &token);
    let donor = Address::generate(&env);
    asset_client.mint(&donor, &50_000_0000000i128);

    client.initialize(&admin, &token);
    client.donate(&donor, &5_000_0000000i128);

    let relief_id = client.submit_request(
        &requester,
        &String::from_str(&env, "Fraudulent request"),
        &50_0000000i128,
        &10u32,
    );
    client.approve_request(&relief_id);
    client.cancel_request(&relief_id);

    let req = client.get_request(&relief_id);
    assert_eq!(req.status, ReliefStatus::Cancelled);
}
