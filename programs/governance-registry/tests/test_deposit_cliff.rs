use anchor_lang::prelude::SolanaSysvar;
use anchor_spl::token::TokenAccount;
use program_test::*;
use solana_program_test::*;
use solana_sdk::{pubkey::Pubkey, signature::Keypair, signer::Signer, transport::TransportError};

mod program_test;

struct Balances {
    token: u64,
    vault: u64,
    deposit: u64,
    voter_weight: u64,
}

async fn balances(
    context: &TestContext,
    registrar: &RegistrarCookie,
    address: Pubkey,
    voter: &VoterCookie,
    rate: &ExchangeRateCookie,
    deposit_id: u8,
) -> Balances {
    // Advance slots to avoid caching of the UpdateVoterWeightRecord call
    // TODO: Is this something that could be an issue on a live node?
    context.solana.advance_clock_by_slots(2).await;

    let token = context.solana.token_account_balance(address).await;
    let vault = rate.vault_balance(&context.solana).await;
    let deposit = voter.deposit_amount(&context.solana, deposit_id).await;
    let vwr = context
        .addin
        .update_voter_weight_record(&registrar, &voter)
        .await
        .unwrap();
    Balances {
        token,
        vault,
        deposit,
        voter_weight: vwr.voter_weight,
    }
}

#[allow(unaligned_references)]
#[tokio::test]
async fn test_deposit_cliff() -> Result<(), TransportError> {
    let context = TestContext::new().await;
    let addin = &context.addin;

    let payer = &context.users[0].key;
    let realm_authority = Keypair::new();
    let realm = context
        .governance
        .create_realm(
            "testrealm",
            realm_authority.pubkey(),
            &context.mints[0],
            &payer,
            &context.addin.program_id,
        )
        .await;

    let voter_authority = &context.users[1].key;
    let token_owner_record = realm
        .create_token_owner_record(voter_authority.pubkey(), &payer)
        .await;

    let registrar = addin.create_registrar(&realm, payer).await;
    let mngo_rate = addin
        .create_exchange_rate(&registrar, &realm_authority, payer, 0, &context.mints[0], 1)
        .await;

    let voter = addin
        .create_voter(&registrar, &voter_authority, &payer)
        .await;

    let reference_account = context.users[1].token_accounts[0];
    let get_balances = |depot_id| {
        balances(
            &context,
            &registrar,
            reference_account,
            &voter,
            &mngo_rate,
            depot_id,
        )
    };
    let withdraw = |amount: u64| {
        addin.withdraw(
            &registrar,
            &voter,
            &token_owner_record,
            &mngo_rate,
            &voter_authority,
            reference_account,
            0,
            amount,
        )
    };
    let update_deposit = |amount: u64| {
        addin.update_deposit(
            &registrar,
            &voter,
            &mngo_rate,
            &voter_authority,
            reference_account,
            0,
            amount,
        )
    };

    // test deposit and withdraw

    let initial = get_balances(0).await;
    assert_eq!(initial.vault, 0);
    assert_eq!(initial.deposit, 0);

    addin
        .create_deposit(
            &registrar,
            &voter,
            &mngo_rate,
            &voter_authority,
            reference_account,
            governance_registry::account::LockupKind::Cliff,
            9000,
            3, // days
        )
        .await
        .unwrap();

    let after_deposit = get_balances(0).await;
    assert_eq!(initial.token, after_deposit.token + after_deposit.vault);
    assert_eq!(after_deposit.voter_weight, after_deposit.vault);
    assert_eq!(after_deposit.vault, 9000);
    assert_eq!(after_deposit.deposit, 9000);

    // cannot withdraw yet, nothing is vested
    withdraw(1).await.expect_err("nothing vested yet");

    // advance almost three days
    addin
        .set_time_offset(&registrar, &realm_authority, 71 * 60 * 60)
        .await;
    context.solana.advance_clock_by_slots(2).await;

    withdraw(1).await.expect_err("nothing vested yet");

    // deposit some more
    update_deposit(1000).await.unwrap();

    // advance more than three days
    addin
        .set_time_offset(&registrar, &realm_authority, 73 * 60 * 60)
        .await;
    context.solana.advance_clock_by_slots(2).await;

    let after_cliff = get_balances(0).await;
    assert_eq!(initial.token, after_cliff.token + after_cliff.vault);
    assert_eq!(after_cliff.voter_weight, after_cliff.vault);
    assert_eq!(after_cliff.vault, 10000);
    assert_eq!(after_cliff.deposit, 10000);

    // can withdraw everything now
    withdraw(10001).await.expect_err("withdrew too much");
    withdraw(10000).await.unwrap();

    let after_withdraw = get_balances(0).await;
    assert_eq!(initial.token, after_withdraw.token + after_withdraw.vault);
    assert_eq!(after_withdraw.voter_weight, after_withdraw.vault);
    assert_eq!(after_withdraw.vault, 0);
    assert_eq!(after_withdraw.deposit, 0);

    Ok(())
}
