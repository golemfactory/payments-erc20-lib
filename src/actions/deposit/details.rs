use erc20_payment_lib::config::Config;
use erc20_payment_lib::eth::deposit_id_from_nonce;
use erc20_payment_lib::runtime::deposit_details;
use erc20_payment_lib::setup::PaymentSetup;
use erc20_payment_lib_common::err_custom_create;
use erc20_payment_lib_common::error::PaymentError;
use erc20_payment_lib_common::model::DepositId;
use std::str::FromStr;
use structopt::StructOpt;
use web3::types::{Address, U256};

#[derive(StructOpt)]
#[structopt(about = "Show details of given deposit")]
pub struct CheckDepositOptions {
    #[structopt(short = "c", long = "chain-name", default_value = "holesky")]
    pub chain_name: String,

    #[structopt(long = "deposit-id", help = "Deposit id to use")]
    pub deposit_id: Option<String>,

    #[structopt(long = "deposit-nonce", help = "Deposit nonce to use")]
    pub deposit_nonce: Option<u64>,

    #[structopt(long = "deposit-funder", help = "Deposit funder")]
    pub deposit_funder: Option<Address>,
}

pub async fn deposit_details_local(
    check_deposit_options: CheckDepositOptions,
    config: Config,
) -> Result<(), PaymentError> {
    log::info!("Deposit details local...");
    //let public_addr = public_addrs.first().expect("No public address found");
    let chain_cfg =
        config
            .chain
            .get(&check_deposit_options.chain_name)
            .ok_or(err_custom_create!(
                "Chain {} not found in config file",
                check_deposit_options.chain_name
            ))?;

    let payment_setup = PaymentSetup::new_empty(&config)?;
    let web3 = payment_setup.get_provider(chain_cfg.chain_id)?;

    let deposit_id = match (
        check_deposit_options.deposit_id,
        check_deposit_options.deposit_nonce,
    ) {
        (Some(deposit_id), None) => U256::from_str(&deposit_id)
            .map_err(|e| err_custom_create!("Invalid deposit id: {}", e))?,
        (None, Some(deposit_nonce)) => {
            if let Some(funder) = check_deposit_options.deposit_funder {
                deposit_id_from_nonce(funder, deposit_nonce)
            } else {
                return Err(err_custom_create!("Missing required parameter: `deposit_funder` must be provided to calculate deposit id from nonce"));
            }
        }
        (Some(_), Some(_)) => {
            return Err(err_custom_create!("Invalid parameters: only one of `deposit_id` or `deposit_nonce` should be provided to terminate a deposit"));
        }
        (None, None) => {
            return Err(err_custom_create!("Missing required parameters: either `deposit_id` or `deposit_nonce` must be provided to terminate a deposit"));
        }
    };

    let details = deposit_details(
        web3,
        DepositId {
            deposit_id,
            lock_address: chain_cfg
                .lock_contract
                .clone()
                .map(|c| c.address)
                .expect("No lock contract found"),
        },
    )
    .await?;
    println!("{}", serde_json::to_string_pretty(&details).unwrap());
    Ok(())
}
