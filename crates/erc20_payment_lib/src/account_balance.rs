use crate::config::Chain;
use crate::err_from;
use crate::error::ErrorBag;
use crate::error::PaymentError;
use crate::runtime::SharedState;
use crate::setup::PaymentSetup;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;
use structopt::StructOpt;
use web3::types::Address;

#[derive(Clone, StructOpt)]
#[structopt(about = "Payment statistics options")]
#[allow(unused)]
pub struct BalanceOptions2 {
    #[structopt(long = "interval")]
    pub interval: Option<f64>,

    #[structopt(
        long = "debug-loop",
        help = "Run forever in loop (for RPC testing) or active balance monitoring. Set number of desired iterations. 0 means forever."
    )]
    pub debug_loop: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceResult2 {
    pub gas: Option<String>,
    pub gas_decimal: Option<String>,
    pub gas_human: Option<String>,
    pub token: Option<String>,
    pub token_decimal: Option<String>,
    pub token_human: Option<String>,
}

pub async fn test_balance_loop(
    _shared_state: Option<Arc<std::sync::Mutex<SharedState>>>,
    payment_setup: PaymentSetup,
    account_balance_options: BalanceOptions2,
    chain_cfg: &Chain,
) -> Result<(), PaymentError> {
    let web3_pool = payment_setup.get_provider(chain_cfg.chain_id).unwrap();

    let mut number_of_loops = account_balance_options.debug_loop.unwrap_or(1);
    if number_of_loops == 0 {
        number_of_loops = u64::MAX;
    }

    let mut prev_loop_time = std::time::Instant::now();
    let mut job_no = 0_u64;
    loop {
        if job_no >= number_of_loops {
            break;
        }
        log::info!("Getting balance: Job number {}/{}", job_no, number_of_loops);
        if let Some(interval) = account_balance_options.interval {
            let elapsed = prev_loop_time.elapsed();
            if elapsed.as_secs_f64() < interval {
                tokio::time::sleep(std::time::Duration::from_secs_f64(
                    interval - elapsed.as_secs_f64(),
                ))
                .await;
            }
            prev_loop_time = std::time::Instant::now();
        }

        let address = "0x200000000000000000000000".to_string()
            + format!("{:#018x}", job_no).replace("0x", "").as_str();
        let address = Address::from_str(&address).map_err(err_from!())?;

        match web3_pool.clone().eth_balance(address, None).await {
            Ok(balance) => balance,
            Err(err) => {
                log::error!(
                    "Error getting balance for account: {:#x} - {}",
                    address,
                    err
                );
                continue;
            }
        };
        job_no += 1;
    }

    Ok(())
}
