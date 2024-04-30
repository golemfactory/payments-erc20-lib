use std::str::FromStr;
use sqlx::SqlitePool;
use structopt::StructOpt;
use web3::ethabi;
use web3::types::{Address, H256};
use erc20_payment_lib::config::Config;
use erc20_payment_lib::eth::get_attestation_details;
use erc20_payment_lib::setup::PaymentSetup;
use erc20_payment_lib_common::err_custom_create;
use erc20_payment_lib_common::error::PaymentError;
use crate::actions::deposit::close::CloseDepositOptions;

#[derive(StructOpt)]
#[structopt(about = "Check attestation")]
pub struct AttestationCheckOptions {
    #[structopt(short = "c", long = "chain-name", default_value = "sepolia")]
    pub chain_name: String,

    #[structopt(short = "u", long = "uid", help = "Attestation uid to check")]
    pub attestation_id: String,
}

pub async fn check_attestation_local(
    _conn: SqlitePool,
    options: AttestationCheckOptions,
    config: Config,
) -> Result<(), PaymentError> {
    log::info!("Checking attestation...");

    let chain_cfg = config
        .chain
        .get(&options.chain_name)
        .ok_or(err_custom_create!(
            "Chain {} not found in config file",
            options.chain_name
        ))?;


    let decoded_bytes = match hex::decode(options.attestation_id.replace("0x", "")) {
        Ok(bytes) => bytes,
        Err(e) => {
            return Err(err_custom_create!(
                "Failed to decode attestation id: {}",
                e
            ));
        }
    };

    let uid = ethabi::Bytes::from(decoded_bytes);

    let contract = chain_cfg
        .attestation_contract
        .as_ref()
        .ok_or(err_custom_create!(
            "Attestation contract not found in chain {}",
            options.chain_name
        ))?;

    let payment_setup = PaymentSetup::new_empty(&config)?;
    let web3 = payment_setup.get_provider(chain_cfg.chain_id)?;

    let uid = if uid.len() != 32 {
        return Err(err_custom_create!(
            "Invalid attestation id length: {}, expected 32",
            uid.len()
        ));
    } else {
        H256::from_slice(uid.as_slice())
    };

    let res = get_attestation_details(
        web3,
        uid,
        contract.address,
    ).await;




    let attestation = match res {
        Ok(attestation) => {
            log::debug!("Attestation details: {:?}", attestation);
            attestation
        }
        Err(e) => {
            log::error!("Failed to get attestation details: {}", e);
            return Err(err_custom_create!(
                "Failed to get attestation details: {}",
                e
            ));
        }
    };

    println!("{}", serde_json::to_string_pretty(&attestation).map_err(
        |e| err_custom_create!("Failed to serialize attestation details: {}", e)
    )?);



    Ok(())
}