use erc20_payment_lib::config::Config;
use erc20_payment_lib::runtime::{close_deposit, CloseDepositOptionsInt};
use erc20_payment_lib::setup::PaymentSetup;
use erc20_payment_lib_common::err_custom_create;
use erc20_payment_lib_common::error::PaymentError;
use erc20_payment_lib_common::model::DepositId;
use sqlx::SqlitePool;
use std::str::FromStr;
use structopt::StructOpt;
use web3::types::{Address, U256};

#[derive(StructOpt)]
#[structopt(about = "Close deposit if you are spender")]
pub struct CloseDepositOptions {
    #[structopt(short = "c", long = "chain-name", default_value = "holesky")]
    pub chain_name: String,

    #[structopt(long = "address", help = "Address (has to have private key)")]
    pub address: Option<Address>,

    #[structopt(long = "account-no", help = "Address by index (for convenience)")]
    pub account_no: Option<usize>,

    #[structopt(long = "skip-check", help = "Skip check deposit")]
    pub skip_check: bool,

    #[structopt(long = "deposit-id", help = "Deposit id to close")]
    pub deposit_id: String,
}

pub async fn close_deposit_local(
    conn: SqlitePool,
    close_deposit_options: CloseDepositOptions,
    config: Config,
    public_addrs: &[Address],
) -> Result<(), PaymentError> {
    log::info!("Making deposit...");
    let public_addr = if let Some(address) = close_deposit_options.address {
        address
    } else if let Some(account_no) = close_deposit_options.account_no {
        *public_addrs
            .get(account_no)
            .expect("No public adss found with specified account_no")
    } else {
        *public_addrs.first().expect("No public adss found")
    };
    let chain_cfg =
        config
            .chain
            .get(&close_deposit_options.chain_name)
            .ok_or(err_custom_create!(
                "Chain {} not found in config file",
                close_deposit_options.chain_name
            ))?;

    let payment_setup = PaymentSetup::new_empty(&config)?;
    let web3 = payment_setup.get_provider(chain_cfg.chain_id)?;

    let deposit_id = U256::from_str(&close_deposit_options.deposit_id)
        .map_err(|e| err_custom_create!("Invalid deposit id: {}", e))?;

    close_deposit(
        web3,
        &conn,
        chain_cfg.chain_id as u64,
        public_addr,
        CloseDepositOptionsInt {
            deposit_id: DepositId {
                lock_address: chain_cfg
                    .lock_contract
                    .clone()
                    .map(|c| c.address)
                    .expect("No lock contract found"),
                deposit_id,
            },
            skip_deposit_check: close_deposit_options.skip_check,
            token_address: chain_cfg.token.address,
        },
    )
    .await?;
    println!(
        "close_deposit added to queue successfully deposit id: {}",
        deposit_id,
    );
    Ok(())
}
