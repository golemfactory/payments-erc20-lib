use erc20_payment_lib::config;
use erc20_payment_lib::config::{Chain, Config, Engine, MultiContractSettings, RpcSettings, Token};
use erc20_payment_lib_common::create_sqlite_connection;
use erc20_payment_lib_common::utils::get_env_bool_value;
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use sqlx::SqlitePool;
use std::collections::BTreeMap;
use std::env;
use std::path::Path;
use std::str::FromStr;
use web3::types::Address;

pub async fn create_default_config_setup(proxy_url_base: &str, proxy_key: &str) -> config::Config {
    let chain = Chain {
        chain_name: "Dev".to_string(),
        chain_id: 987789,
        rpc_endpoints: vec![RpcSettings {
            names: Some(format!("{}/web3/{}", proxy_url_base, proxy_key)),
            endpoints: Some(format!("{}/web3/{}", proxy_url_base, proxy_key)),
            dns_source: None,
            json_source: None,
            skip_validation: None,
            backup_level: None,
            verify_interval_secs: None,
            min_interval_ms: None,
            max_timeout_ms: None,
            allowed_head_behind_secs: Some(200000000000),
            max_consecutive_errors: None,
        }],
        currency_symbol: "tETH".to_string(),
        priority_fee: Decimal::from_f64(1.1).unwrap(),
        max_fee_per_gas: Decimal::from_f64(500.0).unwrap(),
        token: Token {
            symbol: "tGLM".to_string(),
            address: Address::from_str("0xfff17584d526aba263025eE7fEF517E4A31D4246").unwrap(),
            faucet: None,
        },
        multi_contract: Some(MultiContractSettings {
            address: Address::from_str("0xF9861F83766CD507E0d2749B60d4fD6C68E5B96C").unwrap(),
            max_at_once: 10,
        }),
        wrapper_contract: None,
        mint_contract: None,
        lock_contract: None,
        distributor_contract: None,
        attestation_contract: None,
        schema_registry_contract: None,
        faucet_client: None,
        transaction_timeout: 25,
        confirmation_blocks: 1,
        faucet_eth_amount: Some(Decimal::from_f64(10.0).unwrap()),
        faucet_glm_amount: Some(Decimal::from_f64(20.0).unwrap()),
        block_explorer_url: Some("http://127.0.0.1:4000".to_string()),
        replacement_timeout: Some(1.0),
        external_source_check_interval: None,
    };
    let mut chain_map = BTreeMap::new();
    chain_map.insert("dev".to_string(), chain);
    Config {
        chain: chain_map,
        engine: Engine {
            process_interval: 1,
            process_interval_after_error: 1,
            process_interval_after_no_gas_or_token_start: 1,
            process_interval_after_no_gas_or_token_max: 1,
            process_interval_after_no_gas_or_token_increase: 1.0,
            process_interval_after_send: 1,
            report_alive_interval: 1,
            gather_interval: 1,
            mark_as_unrecoverable_after_seconds: None,
            automatic_recover: false,
            gather_at_start: false,
            ignore_deadlines: false,
        },
    }
}

/// Convenient function for use in testing
pub async fn setup_random_memory_sqlite_conn() -> SqlitePool {
    use rand::distributions::Alphanumeric;
    use rand::Rng;

    let rand_string: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(10)
        .map(char::from)
        .collect();

    if get_env_bool_value("ERC20_TESTS_USE_DISK_DB") {
        let db_name = env::var("ERC20_TESTS_OVERRIDE_DB_NAME")
            .unwrap_or(format!("test_{rand_string}.sqlite"));
        log::info!("Using disk db with the name {db_name}");
        create_sqlite_connection(Some(Path::new(db_name.as_str())), None, false, true)
            .await
            .expect("Failed to create sqlite connection")
    } else {
        let db_name =
            env::var("ERC20_TESTS_OVERRIDE_DB_NAME").unwrap_or(format!("mem_{rand_string}"));
        log::info!("Using memory database with the name {db_name}");
        create_sqlite_connection(None, Some(&db_name), false, true)
            .await
            .expect("Failed to create sqlite connection")
    }
}
