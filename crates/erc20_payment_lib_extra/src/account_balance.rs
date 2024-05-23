use erc20_payment_lib::config;
use erc20_payment_lib::eth::{get_balance, GetBalanceArgs};
use erc20_payment_lib::setup::PaymentSetup;
use erc20_payment_lib_common::err_custom_create;
use erc20_payment_lib_common::error::*;
use erc20_payment_lib_common::utils::U256ConvExt;
use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;
use std::str::FromStr;
use stream_rate_limiter::{RateLimitOptions, StreamRateLimitExt};
use structopt::StructOpt;
use web3::types::Address;

#[derive(Clone, StructOpt)]
#[structopt(about = "Payment statistics options")]
pub struct BalanceOptions {
    #[structopt(short = "c", long = "chain-name", default_value = "holesky")]
    pub chain_name: String,

    ///list of accounts separated by comma
    #[structopt(short = "a", long = "accounts")]
    pub accounts: Option<String>,

    #[structopt(long = "hide-gas")]
    pub hide_gas: bool,

    #[structopt(long = "hide-token")]
    pub hide_token: bool,

    #[structopt(long = "block-number")]
    pub block_number: Option<u64>,

    #[structopt(long = "tasks", default_value = "1")]
    pub tasks: usize,

    #[structopt(long = "interval")]
    pub interval: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceResult {
    pub gas: Option<String>,
    pub gas_decimal: Option<String>,
    pub gas_human: Option<String>,
    pub token: Option<String>,
    pub token_decimal: Option<String>,
    pub token_human: Option<String>,
}

pub async fn account_balance(
    account_balance_options: BalanceOptions,
    config: &config::Config,
) -> Result<BTreeMap<String, BalanceResult>, PaymentError> {
    let chain_cfg = config
        .chain
        .get(&account_balance_options.chain_name)
        .ok_or(err_custom_create!(
            "Chain {} not found in config file",
            account_balance_options.chain_name
        ))?;

    let payment_setup = PaymentSetup::new_empty(config)?;

    let web3 = payment_setup.get_provider(chain_cfg.chain_id)?;

    let token = if !account_balance_options.hide_token {
        Some(chain_cfg.token.address)
    } else {
        None
    };

    //deduplicate accounts using hashset
    let accounts = HashSet::<String>::from_iter(
        account_balance_options
            .accounts
            .clone()
            .unwrap()
            .split(',')
            .map(|s| s.trim().to_lowercase()),
    );

    let result_map = Rc::new(RefCell::new(BTreeMap::<String, BalanceResult>::new()));
    let result_map_ = result_map.clone();
    let mut jobs = Vec::new();
    for account in accounts {
        let addr = Address::from_str(&account).map_err(|_| {
            err_custom_create!(
                "Invalid account address: {}",
                account_balance_options.accounts.clone().unwrap()
            )
        })?;
        jobs.push(addr);
    }

    let rate_limit_options = if let Some(interval) = account_balance_options.interval {
        RateLimitOptions::empty().with_min_interval_sec(interval)
    } else {
        RateLimitOptions::empty()
    };

    let wrapper_contract_address = chain_cfg.wrapper_contract.clone().map(|v| v.address);
    stream::iter(0..jobs.len())
        .rate_limit(rate_limit_options)
        .for_each_concurrent(account_balance_options.tasks, |i| {
            let job = jobs[i];
            let result_map = result_map_.clone();
            let web3 = web3.clone();
            async move {
                log::debug!("Getting balance for account: {:#x}", job);
                let args = GetBalanceArgs {
                    address: job,
                    token_address: token,
                    call_with_details: wrapper_contract_address,
                    block_number: None,
                    chain_id: Some(chain_cfg.chain_id as u64),
                };
                let balance = get_balance(
                    web3,
                    args
                )
                .await
                .unwrap();

                let gas_balance = balance.gas_balance.map(|b| b.to_string());
                let token_balance = balance.token_balance.map(|b| b.to_string());
                log::debug!("{:#x} gas: {:?}", job, gas_balance);
                log::debug!("{:#x} token: {:?}", job, token_balance);
                let gas_balance_decimal = balance
                    .gas_balance
                    .map(|v| v.to_eth().unwrap_or_default().to_string());
                let token_balance_decimal = balance
                    .token_balance
                    .map(|v| v.to_eth().unwrap_or_default().to_string());
                let gas_balance_human = gas_balance_decimal.clone().map(|v| {
                    format!(
                        "{:.03} {}",
                        (f64::from_str(&v).unwrap_or(0.0) * 1000.0).floor() / 1000.0,
                        &chain_cfg.currency_symbol
                    )
                });
                let token_balance_human = token_balance_decimal.clone().map(|v| {
                    format!(
                        "{:.03} {}",
                        (f64::from_str(&v).unwrap_or(0.0) * 1000.0).floor() / 1000.0,
                        &chain_cfg.token.symbol
                    )
                });
                result_map.borrow_mut().insert(
                    format!("{:#x}", job),
                    BalanceResult {
                        gas: gas_balance,
                        gas_decimal: gas_balance_decimal,
                        gas_human: gas_balance_human,
                        token: token_balance,
                        token_decimal: token_balance_decimal,
                        token_human: token_balance_human,
                    },
                );
            }
        })
        .await;

    Ok(result_map.take())
}
