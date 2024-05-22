use crate::config::{AdditionalOptions, Config, EasContractSettings, EasSchemaRegistrySettings};
use crate::error::ErrorBag;
use crate::error::PaymentError;

use crate::utils::DecimalConvExt;
use crate::{err_custom_create, err_from};
use erc20_payment_lib_common::DriverEvent;
use erc20_rpc_pool::{
    Web3EndpointParams, Web3ExternalDnsSource, Web3ExternalJsonSource, Web3PoolType, Web3RpcPool,
    Web3RpcSingleParams,
};
use rust_decimal::Decimal;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use uuid::Uuid;
use web3::types::{Address, U256};

#[derive(Clone, Debug)]
pub struct ProviderSetup {
    pub provider: Arc<Web3RpcPool>,
    pub number_of_calls: u64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FaucetSetup {
    pub client_max_eth_allowed: Option<Decimal>,
    pub client_srv: Option<String>,
    pub client_host: Option<String>,
    pub srv_port: Option<u16>,
    pub lookup_domain: Option<String>,
    pub mint_glm_address: Option<Address>,
    pub mint_max_glm_allowed: Option<Decimal>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ChainSetup {
    pub network: String,
    #[serde(skip_serializing)]
    pub provider: Arc<Web3RpcPool>,
    pub chain_name: String,
    pub chain_id: i64,
    pub currency_gas_symbol: String,
    pub currency_glm_symbol: String,
    pub max_fee_per_gas: U256,
    pub priority_fee: U256,
    pub glm_address: Address,
    pub multi_contract_address: Option<Address>,
    pub wrapper_contract_address: Option<Address>,
    pub lock_contract_address: Option<Address>,
    pub distribute_contract_address: Option<Address>,
    pub eas_contract_settings: Option<EasContractSettings>,
    pub eas_schema_registry_settings: Option<EasSchemaRegistrySettings>,
    pub faucet_setup: FaucetSetup,
    pub multi_contract_max_at_once: usize,
    pub transaction_timeout: u64,
    pub skip_multi_contract_check: bool,
    pub confirmation_blocks: u64,
    pub faucet_eth_amount: Option<U256>,
    pub faucet_glm_amount: Option<U256>,
    pub block_explorer_url: Option<String>,
    pub replacement_timeout: Option<f64>,
    pub external_source_check_interval: Option<u64>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExtraOptionsForTesting {
    pub erc20_lib_test_replacement_timeout: Option<Duration>,
    pub balance_check_loop: Option<u64>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PaymentSetup {
    pub chain_setup: BTreeMap<i64, ChainSetup>,
    //pub pub_address: Address,
    pub finish_when_done: bool,
    pub generate_tx_only: bool,
    pub skip_multi_contract_check: bool,
    pub process_interval: u64,
    pub process_interval_after_error: u64,
    pub process_interval_after_send: u64,
    pub process_interval_after_no_gas_or_token_start: u64,
    pub process_interval_after_no_gas_or_token_max: u64,
    pub process_interval_after_no_gas_or_token_increase: f64,
    pub report_alive_interval: u64,
    pub gather_interval: u64,
    pub gather_at_start: bool,
    pub mark_as_unrecoverable_after_seconds: u64,
    pub ignore_deadlines: bool,
    pub automatic_recover: bool,
    pub contract_use_direct_method: bool,
    pub contract_use_unpacked_method: bool,
    pub use_transfer_for_single_payment: bool,
    pub extra_options_for_testing: Option<ExtraOptionsForTesting>,
}

const MARK_AS_UNRECOVERABLE_AFTER_SECONDS: u64 = 300;

fn split_string_by_coma(s: &Option<String>) -> Option<Vec<String>> {
    s.as_ref().map(|s| {
        s.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    })
}

impl PaymentSetup {
    pub fn new(
        config: &Config,
        options: &AdditionalOptions,
        web3_rpc_pool_info: Arc<std::sync::Mutex<BTreeMap<i64, Web3PoolType>>>,
        mpsc_sender: Option<mpsc::Sender<DriverEvent>>,
    ) -> Result<Self, PaymentError> {
        let mut ps = PaymentSetup {
            chain_setup: BTreeMap::new(),
            //pub_address: get_eth_addr_from_secret(secret_key),
            finish_when_done: !options.keep_running,
            generate_tx_only: options.generate_tx_only,
            skip_multi_contract_check: options.skip_multi_contract_check,
            process_interval: config.engine.process_interval,
            process_interval_after_error: config.engine.process_interval_after_error,
            process_interval_after_no_gas_or_token_start: config
                .engine
                .process_interval_after_no_gas_or_token_start,
            process_interval_after_no_gas_or_token_max: config
                .engine
                .process_interval_after_no_gas_or_token_max,
            process_interval_after_no_gas_or_token_increase: config
                .engine
                .process_interval_after_no_gas_or_token_increase,
            process_interval_after_send: config.engine.process_interval_after_send,
            report_alive_interval: config.engine.report_alive_interval,
            gather_interval: config.engine.gather_interval,
            gather_at_start: config.engine.gather_at_start,
            mark_as_unrecoverable_after_seconds: config
                .engine
                .mark_as_unrecoverable_after_seconds
                .unwrap_or(MARK_AS_UNRECOVERABLE_AFTER_SECONDS),
            ignore_deadlines: config.engine.ignore_deadlines,
            automatic_recover: config.engine.automatic_recover,
            contract_use_direct_method: false,
            contract_use_unpacked_method: false,
            extra_options_for_testing: None,
            use_transfer_for_single_payment: true,
        };
        for chain_config in &config.chain {
            let mut single_endpoints = Vec::new();
            let mut json_sources = Vec::new();
            let mut dns_sources = Vec::new();
            for rpc_settings in &chain_config.1.rpc_endpoints {
                let endpoint_names = split_string_by_coma(&rpc_settings.names).unwrap_or_default();
                let max_head_behind_secs = rpc_settings.allowed_head_behind_secs.unwrap_or(120);
                let max_head_behind_secs = if max_head_behind_secs < 0 {
                    None
                } else {
                    Some(max_head_behind_secs as u64)
                };
                if let Some(endpoints) = split_string_by_coma(&rpc_settings.endpoints) {
                    for (idx, endpoint) in endpoints.iter().enumerate() {
                        let endpoint = Web3RpcSingleParams {
                            chain_id: chain_config.1.chain_id as u64,
                            endpoint: endpoint.clone(),
                            name: endpoint_names.get(idx).unwrap_or(&endpoint.clone()).clone(),
                            web3_endpoint_params: Web3EndpointParams {
                                backup_level: rpc_settings.backup_level.unwrap_or(0),
                                skip_validation: rpc_settings.skip_validation.unwrap_or(false),
                                verify_interval_secs: rpc_settings
                                    .verify_interval_secs
                                    .unwrap_or(120),
                                max_response_time_ms: rpc_settings.max_timeout_ms.unwrap_or(10000),
                                max_head_behind_secs,
                                max_number_of_consecutive_errors: rpc_settings
                                    .max_consecutive_errors
                                    .unwrap_or(5),
                                min_interval_requests_ms: rpc_settings.min_interval_ms,
                            },
                            source_id: None,
                        };
                        single_endpoints.push(endpoint);
                    }
                } else if let Some(dns_source) = &rpc_settings.dns_source {
                    dns_sources.push(Web3ExternalDnsSource {
                        chain_id: chain_config.1.chain_id as u64,
                        unique_source_id: Uuid::new_v4(),
                        dns_url: dns_source.clone(),
                        endpoint_params: Web3EndpointParams {
                            backup_level: rpc_settings.backup_level.unwrap_or(0),
                            skip_validation: rpc_settings.skip_validation.unwrap_or(false),
                            verify_interval_secs: rpc_settings.verify_interval_secs.unwrap_or(120),
                            max_response_time_ms: rpc_settings.max_timeout_ms.unwrap_or(10000),
                            max_head_behind_secs,
                            max_number_of_consecutive_errors: rpc_settings
                                .max_consecutive_errors
                                .unwrap_or(5),
                            min_interval_requests_ms: rpc_settings.min_interval_ms,
                        },
                    });
                } else if let Some(json_source) = &rpc_settings.json_source {
                    json_sources.push(Web3ExternalJsonSource {
                        chain_id: chain_config.1.chain_id as u64,
                        unique_source_id: Uuid::new_v4(),
                        url: json_source.clone(),
                        endpoint_params: Web3EndpointParams {
                            backup_level: rpc_settings.backup_level.unwrap_or(0),
                            skip_validation: rpc_settings.skip_validation.unwrap_or(false),
                            verify_interval_secs: rpc_settings.verify_interval_secs.unwrap_or(120),
                            max_response_time_ms: rpc_settings.max_timeout_ms.unwrap_or(10000),
                            max_head_behind_secs,
                            max_number_of_consecutive_errors: rpc_settings
                                .max_consecutive_errors
                                .unwrap_or(5),
                            min_interval_requests_ms: rpc_settings.min_interval_ms,
                        },
                    });
                }
            }
            let web3_pool = Web3RpcPool::new(
                chain_config.1.chain_id as u64,
                single_endpoints,
                json_sources,
                dns_sources,
                mpsc_sender.as_ref().map(|s| s.downgrade()),
                Duration::from_secs(10),
                Duration::from_secs(chain_config.1.external_source_check_interval.unwrap_or(300)),
            );

            web3_rpc_pool_info
                .lock()
                .unwrap()
                .insert(chain_config.1.chain_id, web3_pool.endpoints.clone());

            let faucet_eth_amount = match &chain_config.1.faucet_eth_amount {
                Some(f) => Some((*f).to_u256_from_eth().map_err(err_from!())?),
                None => None,
            };
            let faucet_glm_amount = match &chain_config.1.faucet_glm_amount {
                Some(f) => Some((*f).to_u256_from_eth().map_err(err_from!())?),
                None => None,
            };

            let faucet_setup = FaucetSetup {
                client_max_eth_allowed: chain_config
                    .1
                    .faucet_client
                    .clone()
                    .map(|fc| fc.max_eth_allowed),
                client_srv: chain_config.1.faucet_client.clone().map(|fc| fc.faucet_srv),
                client_host: chain_config
                    .1
                    .faucet_client
                    .clone()
                    .map(|fc| fc.faucet_host),
                srv_port: chain_config
                    .1
                    .faucet_client
                    .clone()
                    .map(|fc| fc.faucet_srv_port),
                lookup_domain: chain_config
                    .1
                    .faucet_client
                    .clone()
                    .map(|fc| fc.faucet_lookup_domain),
                mint_max_glm_allowed: chain_config
                    .1
                    .mint_contract
                    .clone()
                    .map(|mc| mc.max_glm_allowed),
                mint_glm_address: chain_config.1.mint_contract.clone().map(|mc| mc.address),
            };

            ps.chain_setup.insert(
                chain_config.1.chain_id,
                ChainSetup {
                    network: chain_config.0.clone(),
                    provider: web3_pool.clone(),
                    chain_name: chain_config.1.chain_name.clone(),
                    max_fee_per_gas: chain_config
                        .1
                        .max_fee_per_gas
                        .to_u256_from_gwei()
                        .map_err(err_from!())?,
                    priority_fee: chain_config
                        .1
                        .priority_fee
                        .to_u256_from_gwei()
                        .map_err(err_from!())?,
                    glm_address: chain_config.1.token.address,
                    currency_glm_symbol: chain_config.1.token.symbol.clone(),
                    multi_contract_address: chain_config
                        .1
                        .multi_contract
                        .clone()
                        .map(|m| m.address),
                    wrapper_contract_address: chain_config
                        .1
                        .wrapper_contract
                        .clone()
                        .map(|m| m.address),
                    multi_contract_max_at_once: chain_config
                        .1
                        .multi_contract
                        .clone()
                        .map(|m| m.max_at_once)
                        .unwrap_or(1),
                    lock_contract_address: chain_config.1.lock_contract.clone().map(|m| m.address),
                    distribute_contract_address: chain_config
                        .1
                        .distributor_contract
                        .clone()
                        .map(|m| m.address),
                    eas_contract_settings: chain_config
                        .1
                        .attestation_contract
                        .clone()
                        .map(|m| EasContractSettings { address: m.address }),
                    eas_schema_registry_settings: chain_config
                        .1
                        .schema_registry_contract
                        .clone()
                        .map(|m| EasSchemaRegistrySettings { address: m.address }),
                    faucet_setup,

                    transaction_timeout: chain_config.1.transaction_timeout,
                    skip_multi_contract_check: options.skip_multi_contract_check,
                    confirmation_blocks: chain_config.1.confirmation_blocks,
                    currency_gas_symbol: chain_config.1.currency_symbol.clone(),
                    faucet_eth_amount,
                    faucet_glm_amount,
                    block_explorer_url: chain_config.1.block_explorer_url.clone(),
                    chain_id: chain_config.1.chain_id,
                    replacement_timeout: chain_config.1.replacement_timeout,
                    external_source_check_interval: chain_config.1.external_source_check_interval,
                },
            );
        }
        Ok(ps)
    }

    pub fn new_empty(config: &Config) -> Result<Self, PaymentError> {
        PaymentSetup::new(
            config,
            &AdditionalOptions::default(),
            Arc::new(std::sync::Mutex::new(BTreeMap::new())),
            None,
        )
    }

    pub fn get_provider(&self, chain_id: i64) -> Result<Arc<Web3RpcPool>, PaymentError> {
        let chain_setup = self
            .chain_setup
            .get(&chain_id)
            .ok_or_else(|| err_custom_create!("No chain setup for chain id: {}", chain_id))?;

        Ok(chain_setup.provider.clone())
    }
}
