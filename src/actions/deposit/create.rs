use crate::actions::check_address_name;
use chrono::Utc;
use erc20_payment_lib::config::Config;
use erc20_payment_lib::eth::{check_allowance, deposit_id_from_nonce};
use erc20_payment_lib::process_allowance;
use erc20_payment_lib::runtime::{make_deposit, CreateDepositOptionsInt};
use erc20_payment_lib::setup::PaymentSetup;
use erc20_payment_lib::signer::PrivateKeySigner;
use erc20_payment_lib::utils::DecimalConvExt;
use erc20_payment_lib_common::error::ErrorBag;
use erc20_payment_lib_common::error::{AllowanceRequest, PaymentError};
use erc20_payment_lib_common::{err_custom_create, err_from};
use rand::Rng;
use sqlx::SqlitePool;
use std::sync::Arc;
use structopt::StructOpt;
use web3::types::{Address, U256};

#[derive(StructOpt)]
#[structopt(about = "Create deposit for use by spender")]
pub struct CreateDepositOptions {
    #[structopt(short = "c", long = "chain-name", default_value = "holesky")]
    pub chain_name: String,

    #[structopt(long = "address", help = "Address (has to have private key)")]
    pub address: Option<Address>,

    #[structopt(long = "account-no", help = "Address by index (for convenience)")]
    pub account_no: Option<usize>,

    #[structopt(
        long = "spender",
        help = "Specify spender that is allowed to spend allocated tokens"
    )]
    pub spender: String,

    #[structopt(
        short = "a",
        long = "amount",
        help = "Amount (decimal, full precision, i.e. 0.01)"
    )]
    pub amount: Option<rust_decimal::Decimal>,

    #[structopt(
        long = "fee-amount",
        help = "Fee Amount (decimal, full precision, i.e. 0.01)"
    )]
    pub fee_amount: Option<rust_decimal::Decimal>,

    #[structopt(long = "all", help = "Allocate all available tokens")]
    pub allocate_all: bool,

    #[structopt(long = "skip-balance", help = "Skip balance check")]
    pub skip_balance_check: bool,

    #[structopt(long = "block-until", help = "Block until specified date")]
    pub block_until: Option<chrono::DateTime<Utc>>,

    #[structopt(long = "block-for", help = "Block for number of seconds")]
    pub block_for: Option<u64>,

    #[structopt(
        long = "deposit-nonce",
        help = "Deposit nonce to use. If not specified, new deposit id will be generated"
    )]
    pub deposit_nonce: Option<u64>,

    #[structopt(
        long = "lock-contract",
        help = "Lock contract address (if not specified, it will be taken from config)"
    )]
    pub lock_contract: Option<Address>,

    #[structopt(long = "skip-allowance", help = "Skip allowance check")]
    pub skip_allowance: bool,
}

pub async fn make_deposit_local(
    conn: SqlitePool,
    make_deposit_options: CreateDepositOptions,
    config: Config,
    public_addrs: &[Address],
    signer: PrivateKeySigner,
) -> Result<(), PaymentError> {
    log::info!("Making deposit...");
    let public_addr = if let Some(address) = make_deposit_options.address {
        address
    } else if let Some(account_no) = make_deposit_options.account_no {
        *public_addrs
            .get(account_no)
            .expect("No public adss found with specified account_no")
    } else {
        *public_addrs.first().expect("No public adss found")
    };
    let chain_cfg =
        config
            .chain
            .get(&make_deposit_options.chain_name)
            .ok_or(err_custom_create!(
                "Chain {} not found in config file",
                make_deposit_options.chain_name
            ))?;

    let lock_contract = if let Some(lock_contract) = make_deposit_options.lock_contract {
        lock_contract
    } else {
        chain_cfg
            .lock_contract
            .clone()
            .map(|c| c.address)
            .expect("No lock contract found")
    };

    if make_deposit_options.block_for.is_some() && make_deposit_options.block_until.is_some() {
        return Err(err_custom_create!(
            "Cannot specify both block-for and block-until"
        ));
    }

    let timestamp = if let Some(block_for) = make_deposit_options.block_for {
        let now = Utc::now();
        let date_fut =
            now + chrono::Duration::try_seconds(block_for as i64).expect("Invalid value block_for");
        date_fut.timestamp() as u64
    } else if let Some(block_until) = make_deposit_options.block_until {
        block_until.timestamp() as u64
    } else {
        let now = Utc::now();
        now.timestamp() as u64
    };

    let payment_setup = PaymentSetup::new_empty(&config)?;
    let web3 = payment_setup.get_provider(chain_cfg.chain_id)?;

    if !make_deposit_options.skip_allowance {
        let allowance = check_allowance(
            web3.clone(),
            public_addr,
            chain_cfg.token.address,
            lock_contract,
        )
        .await?;

        if (make_deposit_options.fee_amount.unwrap_or_default()
            + make_deposit_options.amount.unwrap_or_default())
        .to_u256_from_eth()
        .map_err(err_from!())?
            > allowance
        {
            let allowance_request = AllowanceRequest {
                owner: format!("{:#x}", public_addr),
                token_addr: format!("{:#x}", chain_cfg.token.address),
                spender_addr: format!("{:#x}", lock_contract),
                chain_id: chain_cfg.chain_id,
                amount: U256::MAX,
            };

            let _ = process_allowance(
                &conn.clone(),
                &payment_setup,
                &allowance_request,
                Arc::new(Box::new(signer)),
                None,
            )
            .await;
            /*return Err(err_custom_create!(
                "Not enough allowance, required: {}, available: {}",
                deposit_tokens_options.amount.unwrap(),
                allowance
            ));*/
        }
    }

    let deposit_nonce = make_deposit_options.deposit_nonce.unwrap_or_else(|| {
        let mut rng = rand::thread_rng();
        rng.gen::<u64>()
    });

    let spender = check_address_name(make_deposit_options.spender.as_str()).map_err(|err| {
        err_custom_create!(
            "Cannot parse spender address {} {}",
            make_deposit_options.spender.as_str(),
            err
        )
    })?;

    make_deposit(
        web3,
        &conn,
        chain_cfg.chain_id as u64,
        public_addr,
        chain_cfg.token.address,
        CreateDepositOptionsInt {
            lock_contract_address: lock_contract,
            spender,
            skip_balance_check: make_deposit_options.skip_balance_check,
            amount: make_deposit_options.amount,
            fee_amount: make_deposit_options.fee_amount,
            allocate_all: make_deposit_options.allocate_all,
            deposit_nonce,
            timestamp,
        },
    )
    .await?;

    let deposit_id = deposit_id_from_nonce(public_addr, deposit_nonce);
    println!(
        "make_deposit added to queue successfully nonce: {}, deposit_id: {:#x}",
        deposit_nonce, deposit_id
    );
    Ok(())
}
