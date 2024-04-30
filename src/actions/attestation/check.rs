use sqlx::SqlitePool;
use structopt::StructOpt;
use web3::ethabi;

use erc20_payment_lib::config::Config;
use erc20_payment_lib::eth::{get_attestation_details, get_schema_details};
use erc20_payment_lib::setup::PaymentSetup;
use erc20_payment_lib_common::err_custom_create;
use erc20_payment_lib_common::error::PaymentError;
use web3::types::H256;

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
            return Err(err_custom_create!("Failed to decode attestation id: {}", e));
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

    let schema_contract = chain_cfg
        .schema_registry_contract
        .as_ref()
        .ok_or(err_custom_create!(
            "Attestation schema contract not found in chain {}",
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
    log::info!("Querying attestation contract: {:#x}", contract.address);

    let attestation = match get_attestation_details(web3.clone(), uid, contract.address).await {
        Ok(Some(attestation)) => attestation,
        Ok(None) => {
            return Err(err_custom_create!(
                "Attestation with uid: {:#x} not found on chain {}",
                uid,
                options.chain_name
            ));
        }
        Err(e) => {
            log::error!("Failed to get attestation details: {}", e);
            return Err(err_custom_create!(
                "Failed to get attestation details: {}",
                e
            ));
        }
    };

    let attestation_schema =
        match get_schema_details(web3, attestation.schema, schema_contract.address).await {
            Ok(attestation_schema) => attestation_schema,
            Err(e) => {
                log::error!("Failed to get attestation details: {}", e);
                return Err(err_custom_create!(
                    "Failed to get attestation details: {}",
                    e
                ));
            }
        };

    log::info!("Querying schema contract: {:#x}", schema_contract.address);

    println!(
        "attestation: {}",
        serde_json::to_string_pretty(&attestation)
            .map_err(|e| err_custom_create!("Failed to serialize attestation details: {}", e))?
    );

    println!(
        "schema: {}",
        serde_json::to_string_pretty(&attestation_schema)
            .map_err(|e| err_custom_create!("Failed to serialize attestation details: {}", e))?
    );

    let items = attestation_schema.schema.split(',').collect::<Vec<&str>>();
    log::debug!("There are {} items in the schema", items.len());
    let mut param_types = Vec::new();
    let mut param_names = Vec::new();
    for item in items {
        let items2 = item.trim().split(' ').collect::<Vec<&str>>();
        if items2.len() != 2 {
            log::error!("Invalid item in schema: {}", item);
            return Err(err_custom_create!("Invalid item in schema: {}", item));
        }
        let item_type = items2[0].trim();
        let item_name = items2[1].trim();

        log::debug!("Item name: {}, Item type: {}", item_name, item_type);
        let param_type = ethabi::param_type::Reader::read(item_type)
            .map_err(|e| err_custom_create!("Failed to read param type: {}", e))?;
        param_types.push(param_type);
        param_names.push(item_name);
    }

    let decoded_tokens = ethabi::decode(&param_types, &attestation.data.0)
        .map_err(|e| err_custom_create!("Failed to decode attestation data: {}", e))?;

    for (token, token_name) in decoded_tokens.iter().zip(param_names.iter()) {
        println!("Token {}: {}", token_name, token);
    }
    //println!(attestation_schema.schema);

    Ok(())
}
