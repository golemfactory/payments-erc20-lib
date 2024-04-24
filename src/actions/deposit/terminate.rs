use erc20_payment_lib::config::Config;
use erc20_payment_lib::eth::deposit_id_from_nonce;
use erc20_payment_lib::runtime::{terminate_deposit, TerminateDepositOptionsInt};
use erc20_payment_lib::setup::PaymentSetup;
use erc20_payment_lib_common::err_custom_create;
use erc20_payment_lib_common::error::PaymentError;
use erc20_payment_lib_common::model::DepositId;
use sqlx::SqlitePool;
use std::str::FromStr;
use structopt::StructOpt;
use web3::types::{Address, U256};

#[derive(StructOpt)]
#[structopt(about = "Terminate deposit if you are funder")]
pub struct TerminateDepositOptions {
    #[structopt(short = "c", long = "chain-name", default_value = "holesky")]
    pub chain_name: String,

    #[structopt(long = "address", help = "Address (has to have private key)")]
    pub address: Option<Address>,

    #[structopt(long = "account-no", help = "Address by index (for convenience)")]
    pub account_no: Option<usize>,

    #[structopt(long = "skip-check", help = "Skip check deposit")]
    pub skip_check: bool,

    #[structopt(long = "deposit-id", help = "Deposit id to terminate.")]
    pub deposit_id: Option<String>,

    #[structopt(
        long = "lock-contract",
        help = "Lock contract address (if not specified, it will be taken from config)"
    )]
    pub lock_contract: Option<Address>,

    #[structopt(long = "deposit-nonce", help = "Deposit nonce to terminate.")]
    pub deposit_nonce: Option<u64>,
}

pub async fn terminate_deposit_local(
    conn: SqlitePool,
    terminate_deposit_options: TerminateDepositOptions,
    config: Config,
    public_addrs: &[Address],
) -> Result<(), PaymentError> {
    log::info!("Making deposit...");
    let public_addr = if let Some(address) = terminate_deposit_options.address {
        address
    } else if let Some(account_no) = terminate_deposit_options.account_no {
        *public_addrs
            .get(account_no)
            .expect("No public adss found with specified account_no")
    } else {
        *public_addrs.first().expect("No public adss found")
    };
    let chain_cfg = config
        .chain
        .get(&terminate_deposit_options.chain_name)
        .ok_or(err_custom_create!(
            "Chain {} not found in config file",
            terminate_deposit_options.chain_name
        ))?;

    let lock_contract = if let Some(lock_contract) = terminate_deposit_options.lock_contract {
        lock_contract
    } else {
        chain_cfg
            .lock_contract
            .clone()
            .map(|c| c.address)
            .expect("No lock contract found")
    };

    let payment_setup = PaymentSetup::new_empty(&config)?;
    let web3 = payment_setup.get_provider(chain_cfg.chain_id)?;

    if terminate_deposit_options.deposit_id.is_some()
        && terminate_deposit_options.deposit_nonce.is_some()
    {
        return Err(err_custom_create!("Invalid parameters: only one of `deposit_id` or `deposit_nonce` should be provided to terminate a deposit"));
    }

    let deposit_id = match (
        terminate_deposit_options.deposit_id,
        terminate_deposit_options.deposit_nonce,
    ) {
        (Some(deposit_id), None) => U256::from_str(&deposit_id)
            .map_err(|e| err_custom_create!("Invalid deposit id: {}", e))?,
        (None, Some(deposit_nonce)) => deposit_id_from_nonce(public_addr, deposit_nonce),
        (Some(_), Some(_)) => {
            return Err(err_custom_create!("Invalid parameters: only one of `deposit_id` or `deposit_nonce` should be provided to terminate a deposit"));
        }
        (None, None) => {
            return Err(err_custom_create!("Missing required parameters: either `deposit_id` or `deposit_nonce` must be provided to terminate a deposit"));
        }
    };

    terminate_deposit(
        web3,
        &conn,
        chain_cfg.chain_id as u64,
        public_addr,
        TerminateDepositOptionsInt {
            deposit_id: DepositId {
                deposit_id,
                lock_address: lock_contract,
            },
            skip_deposit_check: terminate_deposit_options.skip_check,
        },
    )
    .await?;
    println!(
        "terminate_deposit added to queue successfully deposit id: {}",
        deposit_id,
    );
    Ok(())
}
