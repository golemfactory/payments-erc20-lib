use serde::{Deserialize, Serialize};
use std::collections::btree_map::BTreeMap as Map;

use rust_decimal::Decimal;
use std::path::Path;

use crate::err_custom_create;
use crate::error::*;
use erc20_payment_lib_common::err_create;
use tokio::fs;
use web3::types::Address;

#[derive(Debug, Clone)]
pub struct AdditionalOptions {
    ///Set to keep running when finished processing transactions
    pub keep_running: bool,
    ///Set to skip running service loop (do not send and process transactions)
    pub skip_service_loop: bool,
    ///Do not send or process transactions, only generate stubs
    pub generate_tx_only: bool,
    ///Skip multi contract check when generating txs
    pub skip_multi_contract_check: bool,
    pub contract_use_direct_method: bool,
    pub contract_use_unpacked_method: bool,
    pub use_transfer_for_single_payment: bool,
}

impl Default for AdditionalOptions {
    fn default() -> Self {
        AdditionalOptions {
            keep_running: true,
            generate_tx_only: false,
            skip_multi_contract_check: false,
            contract_use_direct_method: false,
            contract_use_unpacked_method: false,
            use_transfer_for_single_payment: true,
            skip_service_loop: false,
        }
    }
}

impl AdditionalOptions {
    pub fn keep_running(&mut self, keep_running: bool) -> &mut Self {
        self.keep_running = keep_running;
        self
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct Engine {
    pub process_interval: u64,
    pub process_interval_after_error: u64,
    pub process_interval_after_no_gas_or_token_start: u64,
    pub process_interval_after_no_gas_or_token_max: u64,
    pub process_interval_after_no_gas_or_token_increase: f64,
    pub process_interval_after_send: u64,
    pub report_alive_interval: u64,
    pub gather_interval: u64,
    pub mark_as_unrecoverable_after_seconds: Option<u64>,
    pub gather_at_start: bool,
    pub automatic_recover: bool,
    pub ignore_deadlines: bool,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Config {
    pub chain: Map<String, Chain>,
    pub engine: Engine,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct MultiContractSettings {
    pub address: Address,
    pub max_at_once: usize,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct WrapperContractSettings {
    pub address: Address,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct MintContractSettings {
    pub address: Address,
    pub max_glm_allowed: Decimal,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct LockContractSettings {
    pub address: Address,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct DistributorContractSettings {
    pub address: Address,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct EasContractSettings {
    pub address: Address,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct EasSchemaRegistrySettings {
    pub address: Address,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct FaucetClientSettings {
    pub max_eth_allowed: Decimal,
    pub faucet_srv: String,
    pub faucet_host: String,
    pub faucet_srv_port: u16,
    pub faucet_lookup_domain: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct RpcSettings {
    pub names: Option<String>,
    pub endpoints: Option<String>,
    pub dns_source: Option<String>,
    pub json_source: Option<String>,
    pub skip_validation: Option<bool>,
    pub backup_level: Option<i64>,
    pub verify_interval_secs: Option<u64>,
    pub min_interval_ms: Option<u64>,
    pub max_timeout_ms: Option<u64>,
    pub allowed_head_behind_secs: Option<i64>,
    pub max_consecutive_errors: Option<u64>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct Chain {
    pub chain_name: String,
    pub chain_id: i64,
    pub rpc_endpoints: Vec<RpcSettings>,
    pub currency_symbol: String,
    pub priority_fee: Decimal,
    pub max_fee_per_gas: Decimal,
    pub token: Token,
    pub multi_contract: Option<MultiContractSettings>,
    pub wrapper_contract: Option<WrapperContractSettings>,
    pub mint_contract: Option<MintContractSettings>,
    pub lock_contract: Option<LockContractSettings>,
    pub distributor_contract: Option<DistributorContractSettings>,
    pub attestation_contract: Option<EasContractSettings>,
    pub schema_registry_contract: Option<EasSchemaRegistrySettings>,
    pub faucet_client: Option<FaucetClientSettings>,
    pub transaction_timeout: u64,
    pub confirmation_blocks: u64,
    pub faucet_eth_amount: Option<Decimal>,
    pub faucet_glm_amount: Option<Decimal>,
    pub block_explorer_url: Option<String>,
    pub replacement_timeout: Option<f64>,
    pub external_source_check_interval: Option<u64>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Token {
    pub symbol: String,
    pub address: Address,
    pub faucet: Option<Address>,
}

impl Config {
    pub fn default_config_str() -> &'static str {
        //include config.toml
        let config = include_str!("../config-payments.toml");
        config
    }

    pub fn default_config() -> Self {
        //include config.toml
        Self::load_from_str(Self::default_config_str()).unwrap()
    }

    pub fn load_from_str(str: &str) -> Result<Self, PaymentError> {
        match toml::from_str(str) {
            Ok(config) => Ok(config),
            Err(e) => Err(err_custom_create!("Failed to parse toml {}: {}", str, e)),
        }
    }

    pub async fn load<P: AsRef<Path>>(path: P) -> Result<Self, PaymentError> {
        match toml::from_str(&String::from_utf8_lossy(&fs::read(&path).await.map_err(
            |e| match e.kind() {
                std::io::ErrorKind::NotFound => {
                    err_create!(e)
                }
                _ => {
                    err_custom_create!(
                        "Failed to read config file {}. Error {}",
                        path.as_ref().display(),
                        e
                    )
                }
            },
        )?)) {
            Ok(config) => Ok(config),
            Err(e) => Err(err_custom_create!(
                "Failed to parse toml {}: {}",
                path.as_ref().display(),
                e
            )),
        }
    }

    pub async fn change_rpc_endpoints(
        &mut self,
        chain: &str,
        rpc_endpoints: Vec<RpcSettings>,
    ) -> Result<(), PaymentError> {
        self.chain
            .get_mut(chain)
            .ok_or(err_custom_create!("Chain {} not found", chain))?
            .rpc_endpoints = rpc_endpoints;
        Ok(())
    }

    pub async fn change_max_fee(
        &mut self,
        chain: &str,
        max_fee_per_gas: Decimal,
    ) -> Result<(), PaymentError> {
        self.chain
            .get_mut(chain)
            .ok_or(err_custom_create!("Chain {} not found", chain))?
            .max_fee_per_gas = max_fee_per_gas;
        Ok(())
    }
}
