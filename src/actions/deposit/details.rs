use erc20_payment_lib::config::Config;
use erc20_payment_lib::eth::deposit_id_from_nonce;
use erc20_payment_lib::runtime::{deposit_details, validate_deposit, ValidateDepositResult};
use erc20_payment_lib::setup::PaymentSetup;
use erc20_payment_lib_common::err_custom_create;
use erc20_payment_lib_common::error::PaymentError;
use erc20_payment_lib_common::model::DepositId;
use std::collections::BTreeMap;
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

    #[structopt(
        long = "lock-contract",
        help = "Lock contract address (if not specified, it will be taken from config)"
    )]
    pub lock_contract: Option<Address>,

    #[structopt(long = "validate", help = "Perform extra parameters validation")]
    pub encoded_validation_params: Option<String>,

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

    let lock_contract = if let Some(lock_contract) = check_deposit_options.lock_contract {
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
        web3.clone(),
        DepositId {
            deposit_id,
            lock_address: lock_contract,
        },
    )
    .await?;
    println!("{}", serde_json::to_string_pretty(&details).unwrap());

    if let Some(encoded_params) = check_deposit_options.encoded_validation_params {
        let params = encoded_params.split(';');

        let mut parameters = BTreeMap::<String, String>::new();
        for param in params.into_iter() {
            //split string in two parts
            let (name, value) = param.split_at(param.find('=').ok_or_else(|| {
                err_custom_create!("Expected parameter format \"param1 = value1; param2 = value2\"")
            })?);
            let name = name
                .trim_matches(|c: char| c.is_ascii_whitespace())
                .to_string();
            let value = value
                .trim_matches(|c: char| c == '=' || c.is_ascii_whitespace())
                .to_string();
            if name.is_empty() {
                return Err(err_custom_create!(
                    "Invalid parameter format: name is empty"
                ));
            }
            if value.is_empty() {
                return Err(err_custom_create!(
                    "Invalid parameter format: value is empty"
                ));
            }
            if parameters.get(&name).is_some() {
                return Err(err_custom_create!(
                    "Invalid parameter format: parameter {} is duplicated",
                    name
                ));
            }
            parameters.insert(name.to_string(), value.to_string());
        }

        let validate_result = validate_deposit(
            web3,
            DepositId {
                deposit_id,
                lock_address: lock_contract,
            },
            parameters,
        )
        .await?;
        match validate_result {
            ValidateDepositResult::Valid => {
                print!("Deposit is valid");
            }
            ValidateDepositResult::Invalid(err_str) => {
                print!("Deposit is invalid: {}", err_str);
            }
        }
    }

    Ok(())
}
