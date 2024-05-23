use crate::error::PaymentError;
use crate::error::*;
use crate::{err_create, err_custom_create, err_from};
use erc20_payment_lib_common::model::TxDbObj;
use erc20_payment_lib_common::ops::{
    delete_tx, get_transaction, get_transaction_highest_nonce, insert_tx, remap_allowance_tx,
    remap_token_transfer_tx, update_processing_and_first_processed_tx, update_tx,
    update_tx_stuck_date,
};
use erc20_payment_lib_common::{
    CantSignContent, DriverEvent, DriverEventContent, GasLowInfo, NoGasDetails,
    TransactionStuckReason,
};
use rust_decimal::prelude::Zero;
use rust_decimal::Decimal;
use sqlx::SqlitePool;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use web3::transports::Http;
use web3::types::{Address, BlockId, BlockNumber, U256, U64};
use web3::Web3;

use crate::eth::get_transaction_count;
use crate::runtime::{remove_transaction_force, send_driver_event, SharedState};
use crate::setup::PaymentSetup;
use crate::signer::Signer;
use crate::transaction::check_transaction;
use crate::transaction::find_receipt;
use crate::transaction::send_transaction;
use crate::transaction::sign_transaction_with_callback;
use crate::utils::{
    datetime_from_u256_timestamp, get_env_bool_value, DecimalConvExt, StringConvExt, U256ConvExt,
};

#[derive(Debug)]
pub enum ProcessTransactionResult {
    Confirmed,
    Replaced,
    NeedRetry(String),
    InternalError(String),
    DoNotSave,
    DoNotSaveWaitForGasOrToken,
    Unknown,
}

#[allow(dead_code)]
pub async fn get_provider(url: &str) -> Result<Web3<Http>, PaymentError> {
    let transport = web3::transports::Http::new(url).map_err(err_from!())?;
    let web3 = web3::Web3::new(transport);
    Ok(web3)
}

pub async fn process_transaction(
    event_sender: Option<tokio::sync::mpsc::Sender<DriverEvent>>,
    shared_state: Arc<std::sync::Mutex<SharedState>>,
    conn: &SqlitePool,
    web3_tx_dao: &mut TxDbObj,
    payment_setup: &PaymentSetup,
    signer: Arc<Box<dyn Signer + Send + Sync + 'static>>,
    wait_for_confirmation: bool,
) -> Result<(TxDbObj, ProcessTransactionResult), PaymentError> {
    let chain_id = web3_tx_dao.chain_id;
    let Some(chain_setup) = payment_setup.chain_setup.get(&chain_id) else {
        return Ok((web3_tx_dao.clone(), ProcessTransactionResult::DoNotSave));
    };

    let is_polygon_eco_mode = chain_setup.chain_id == 137 && get_env_bool_value("POLYGON_ECO_MODE");

    let web3 = payment_setup.get_provider(chain_id).map_err(|_e| {
        err_create!(TransactionFailedError::new(&format!(
            "Failed to get provider for chain id: {chain_id}"
        )))
    })?;
    let from_addr = Address::from_str(&web3_tx_dao.from_addr)
        .map_err(|_e| err_create!(TransactionFailedError::new("Failed to parse from_addr")))?;

    if let Err(err) = signer.check_if_sign_possible(from_addr).await {
        send_driver_event(
            &event_sender,
            DriverEventContent::CantSign(CantSignContent::Tx(web3_tx_dao.clone())),
        )
        .await;

        return Err(err_create!(TransactionFailedError::new(&format!(
            "Sign won't be possible for given address: {from_addr}, error: {err:?}"
        ))));
    }

    let transaction_nonce = if let Some(nonce) = web3_tx_dao.nonce {
        nonce
    } else {
        shared_state
            .lock()
            .unwrap()
            .set_tx_message(web3_tx_dao.id, "Obtaining transaction nonce".to_string());

        let nonce = get_transaction_count(from_addr, web3.clone(), false)
            .await
            .map_err(|err| {
                err_custom_create!(
                    "Web3 RPC endpoint failing for network {}(chainId: {}): {}",
                    chain_setup.chain_name,
                    chain_setup.chain_id,
                    err
                )
            })? as i64;

        // do not trust blockchain for returning proper nonce, it can be lower than real one
        // potentially it could be higher, but it is very hard to work around it and hopefully it won't happen

        let db_nonce = get_transaction_highest_nonce(conn, chain_id, &web3_tx_dao.from_addr)
            .await
            .map_err(err_from!())?;

        if let Some(db_nonce) = db_nonce {
            let db_nonce = db_nonce + 1; //normalize to blockchain nonce
            if nonce != db_nonce {
                log::warn!(
                    "Nonce mismatch for address: {}, blockchain nonce: {}, db nonce: {}",
                    web3_tx_dao.from_addr,
                    nonce,
                    db_nonce
                );
                if nonce > db_nonce {
                    log::warn!("Blockchain nonce is higher than db nonce, using blockchain nonce (probably external payment)");
                } else {
                    log::warn!(
                        "Blockchain nonce is lower than db nonce, blockchain is not updated yet"
                    );
                    return Err(err_custom_create!(
                        "Blockchain nonce is lower than db nonce, blockchain is not updated yet"
                    ));
                }
            }
        }
        let mut max_fee_per_gas = if let Some(max_fee_per_gas) = &web3_tx_dao.max_fee_per_gas {
            max_fee_per_gas.to_u256().map_err(err_from!())?
        } else {
            chain_setup.max_fee_per_gas
        };
        let max_priority_fee = if let Some(priority_fee) = &web3_tx_dao.priority_fee {
            priority_fee.to_u256().map_err(err_from!())?
        } else {
            chain_setup.priority_fee
        };

        if is_polygon_eco_mode {
            let blockchain_gas_price = web3
                .clone()
                .eth_block(BlockId::Number(BlockNumber::Latest))
                .await
                .map_err(err_from!())?
                .ok_or(err_create!(TransactionFailedError::new(
                    "Failed to get latest block from RPC node"
                )))?
                .base_fee_per_gas
                .ok_or(err_create!(TransactionFailedError::new(
                    "Failed to get base_fee_per_gas from RPC node"
                )))?;

            let extra_gas = if let Ok(extra_gas) = std::env::var("POLYGON_ECO_MODE_EXTRA_GAS") {
                let mut extra_gas = Decimal::from_str_exact(&extra_gas).map_err(|err| {
                    err_custom_create!("POLYGON_ECO_MODE_EXTRA_GAS has to be decimal format {err}")
                })?;

                if extra_gas > Decimal::from(30) {
                    log::warn!("Extra gas is too high, setting to 30");
                    extra_gas = Decimal::from(30);
                }
                if extra_gas < Decimal::from(-10) {
                    log::warn!("Extra gas is too low, setting to -10");
                    extra_gas = Decimal::from(-10);
                }
                extra_gas
            } else {
                Decimal::zero()
            };

            let mut new_target_gas_u256 = if extra_gas >= Decimal::zero() {
                let extra_gas_u256 = extra_gas.to_u256_from_gwei().map_err(err_from!())?;
                blockchain_gas_price + extra_gas_u256
            } else {
                let extra_gas_u256 = extra_gas.to_u256_from_gwei().map_err(err_from!())?;
                let min_base_price = U256::from(1_000_000_000u64);

                let mut new_target_gas_u256 = blockchain_gas_price - extra_gas_u256;
                if blockchain_gas_price < min_base_price + extra_gas_u256 {
                    new_target_gas_u256 = min_base_price;
                }
                new_target_gas_u256
            };

            //max_fee_per_gas cannot be lower than priority fee
            if new_target_gas_u256 < max_priority_fee {
                new_target_gas_u256 = max_priority_fee;
            }

            if new_target_gas_u256 * 11 < max_fee_per_gas * 10 {
                log::warn!(
                    "Eco mode activated. Sending transaction with lower base fee. Blockchain base fee: {} Gwei, Tx base fee: {} Gwei",
                    blockchain_gas_price.to_gwei().map_err(err_from!())?,
                    new_target_gas_u256.to_gwei().map_err(err_from!())?,
                );

                max_fee_per_gas = new_target_gas_u256;
            }
        }
        web3_tx_dao.max_fee_per_gas = Some(max_fee_per_gas.to_string());
        web3_tx_dao.priority_fee = Some(max_priority_fee.to_string());
        web3_tx_dao.nonce = Some(nonce);
        nonce
    };

    //timeout transaction when it is not confirmed after transaction_timeout seconds
    if let Some(first_processed) = web3_tx_dao.first_processed {
        let now = chrono::Utc::now();
        let diff = now - first_processed;
        if diff.num_seconds() < -10 {
            log::warn!("Time changed?? time diff lower than 0");
        }
        if diff.num_seconds() > chain_setup.transaction_timeout as i64 {
            log::warn!("Detected transaction timeout for tx id: {}", web3_tx_dao.id);
        }
    } else {
        web3_tx_dao.first_processed = Some(chrono::Utc::now());
        web3_tx_dao.processing = 1;
        update_processing_and_first_processed_tx(conn, web3_tx_dao)
            .await
            .map_err(err_from!())?;
    }

    if web3_tx_dao.signed_raw_data.is_none() {
        //if transaction is replacement it does not need checking
        if web3_tx_dao.orig_tx_id.is_none() {
            shared_state
                .lock()
                .unwrap()
                .set_tx_message(web3_tx_dao.id, "Checking transaction".to_string());
            log::info!("Check and estimate gas for tx id: {}", web3_tx_dao.id);
            match check_transaction(
                &event_sender,
                conn,
                chain_setup.glm_address,
                web3.clone(),
                web3_tx_dao,
                chain_setup.wrapper_contract_address,
            )
            .await
            {
                Ok(res) => {
                    let Some(res) = res else {
                        //check cannot be done right now
                        return Ok((
                            web3_tx_dao.clone(),
                            ProcessTransactionResult::DoNotSaveWaitForGasOrToken,
                        ));
                    };
                    let gas_balance = web3
                        .clone()
                        .eth_balance(from_addr, None)
                        .await
                        .map_err(err_from!())?;
                    if gas_balance < res {
                        log::warn!(
                            "Gas balance too low for gas {} - vs needed: {}",
                            gas_balance.to_eth().map_err(err_from!())?,
                            res.to_eth().map_err(err_from!())?
                        );
                        send_driver_event(
                            &event_sender,
                            DriverEventContent::TransactionStuck(TransactionStuckReason::NoGas(
                                NoGasDetails {
                                    tx: web3_tx_dao.clone(),
                                    gas_balance: gas_balance.to_eth().map_err(err_from!())?,
                                    gas_needed: res.to_eth().map_err(err_from!())?,
                                },
                            )),
                        )
                        .await;
                        return Ok((
                            web3_tx_dao.clone(),
                            ProcessTransactionResult::DoNotSaveWaitForGasOrToken,
                        ));
                    }
                }
                Err(err) => {
                    let err_msg = format!("{err}");
                    if err_msg
                        .to_lowercase()
                        .contains("insufficient funds for transfer")
                    {
                        log::error!(
                            "Insufficient {} for tx id: {}",
                            chain_setup.currency_gas_symbol,
                            web3_tx_dao.id
                        );
                        return Err(err);
                    }
                    log::error!("Error while checking transaction: {}", err);
                    return Err(err);
                }
            }
            log::debug!("web3_tx_dao after check_transaction: {:?}", web3_tx_dao);
        }
        shared_state
            .lock()
            .unwrap()
            .set_tx_message(web3_tx_dao.id, "Signing transaction".to_string());
        sign_transaction_with_callback(&event_sender, web3_tx_dao, from_addr, signer).await?;
        update_tx(conn, web3_tx_dao).await.map_err(err_from!())?;
    }

    if web3_tx_dao.broadcast_date.is_none() {
        log::info!(
            "Sending transaction with id: {} with nonce {}",
            web3_tx_dao.id,
            transaction_nonce
        );
        shared_state
            .lock()
            .unwrap()
            .set_tx_message(web3_tx_dao.id, "Sending transaction".to_string());
        web3_tx_dao.broadcast_count += 1;
        update_tx(conn, web3_tx_dao).await.map_err(err_from!())?;
        send_transaction(
            conn,
            chain_setup.chain_id,
            chain_setup.glm_address,
            event_sender.clone(),
            web3.clone(),
            web3_tx_dao,
            chain_setup.wrapper_contract_address,
        )
        .await?;
        update_tx(conn, web3_tx_dao).await.map_err(err_from!())?;
        log::info!(
            "Transaction {} sent, tx hash: {}",
            web3_tx_dao.id,
            web3_tx_dao.tx_hash.clone().unwrap_or_default()
        );
        tokio::time::sleep(Duration::from_secs(
            payment_setup.process_interval_after_send,
        ))
        .await;
    }

    if web3_tx_dao.confirm_date.is_some() {
        log::info!("Transaction already confirmed {}", web3_tx_dao.id);
        return Ok((web3_tx_dao.clone(), ProcessTransactionResult::Confirmed));
    }

    loop {
        shared_state
            .lock()
            .unwrap()
            .set_tx_message(web3_tx_dao.id, "Confirmations - checking nonce".to_string());

        log::debug!(
            "Checking latest nonce tx: {}, expected nonce: {}",
            web3_tx_dao.id,
            transaction_nonce + 1
        );
        let latest_nonce = get_transaction_count(from_addr, web3.clone(), false)
            .await
            .map_err(|err| {
                err_custom_create!(
                    "Web3 RPC endpoint failing for network {}(chainId: {}): {}",
                    chain_setup.chain_name,
                    chain_setup.chain_id,
                    err
                )
            })?;

        let current_block_number = web3
            .clone()
            .eth_block_number()
            .await
            .map_err(|err| {
                err_custom_create!(
                    "Web3 RPC endpoint failing for network {}(chainId: {}): {}",
                    chain_setup.chain_name,
                    chain_setup.chain_id,
                    err
                )
            })?
            .as_u64();

        let db_nonce = web3_tx_dao
            .nonce
            .map(|n| n as u64)
            .ok_or_else(|| err_custom_create!("Nonce should be present in db"))?;

        if latest_nonce > db_nonce {
            shared_state.lock().unwrap().set_tx_message(
                web3_tx_dao.id,
                "Confirmations - checking receipt".to_string(),
            );

            //Normally we have one transaction to check, unless it is replacement transaction then we have to check whole chain
            let mut current_tx = web3_tx_dao.clone();
            let res = loop {
                let res = find_receipt(web3.clone(), &mut current_tx).await?;
                if res.is_some() {
                    //if receipt found then break early, we found our transaction
                    break res;
                }
                if let Some(orig_tx_id) = web3_tx_dao.orig_tx_id {
                    //jump to previous transaction in chain
                    current_tx = get_transaction(conn, orig_tx_id)
                        .await
                        .map_err(err_from!())?;
                } else {
                    break res;
                }
            };

            if let Some(effective_gas_price) = res {
                let Some(block_number) = current_tx.block_number.map(|bn| bn as u64) else {
                    return Err(err_custom_create!(
                        "Block number not found on dao for tx: {}",
                        current_tx.id
                    ));
                };

                log::info!(
                    "Receipt found: tx {} tx_hash: {}",
                    current_tx.id,
                    current_tx.tx_hash.clone().unwrap_or_default()
                );
                if block_number + chain_setup.confirmation_blocks <= current_block_number {
                    current_tx.confirm_date = Some(chrono::Utc::now());
                    log::info!(
                        "Transaction confirmed: tx: {} tx_hash: {}",
                        current_tx.id,
                        current_tx.tx_hash.clone().unwrap_or_default()
                    );

                    if is_polygon_eco_mode {
                        let blockchain_gas_price = web3
                            .eth_block(BlockId::Number(BlockNumber::Number(U64::from(
                                block_number,
                            ))))
                            .await;
                        if let Ok(Some(block)) = blockchain_gas_price {
                            if let Some(base_fee_per_gas_u256) = block.base_fee_per_gas {
                                if effective_gas_price > base_fee_per_gas_u256 {
                                    let base_fee_per_gas =
                                        base_fee_per_gas_u256.to_gwei().unwrap_or_default();
                                    let effective_priority_fee = (effective_gas_price
                                        - base_fee_per_gas_u256)
                                        .to_gwei()
                                        .unwrap_or_default();
                                    log::info!(
                                        "Effective priority fee: {}",
                                        effective_priority_fee
                                    );
                                    log::info!(
                                        "Saved: {:.2} Gwei ({:.1}%)",
                                        Decimal::from(30) - effective_priority_fee,
                                        Decimal::from(100)
                                            * (Decimal::from(30) - effective_priority_fee)
                                            / (base_fee_per_gas + Decimal::from(30))
                                    );
                                }
                            }
                        };
                    }

                    //cleanup txs
                    //let confirmed_tx = current_tx.clone();
                    let mut orig_tx = current_tx.clone();
                    let orig_tx = loop {
                        if let Some(next_tx) = orig_tx.orig_tx_id {
                            let next_tx =
                                get_transaction(conn, next_tx).await.map_err(err_from!())?;
                            orig_tx = next_tx;
                        } else {
                            break orig_tx;
                        }
                    };

                    let mut db_transaction = conn.begin().await.map_err(err_from!())?;
                    if orig_tx.id != current_tx.id {
                        log::info!(
                            "Updating orig tx: {} with confirmed tx: {}",
                            orig_tx.id,
                            current_tx.id
                        );
                        //handle case when transaction is allowance
                        remap_allowance_tx(&mut *db_transaction, orig_tx.id, current_tx.id)
                            .await
                            .map_err(err_from!())?;

                        //handle case when transaction is token transfer
                        remap_token_transfer_tx(&mut *db_transaction, orig_tx.id, current_tx.id)
                            .await
                            .map_err(err_from!())?;
                    }
                    let mut process_tx = web3_tx_dao.clone();
                    let _ = web3_tx_dao; //do not use it later
                    loop {
                        if process_tx.id != current_tx.id {
                            log::info!("Deleting tx: {}", process_tx.id);
                            delete_tx(&mut *db_transaction, process_tx.id)
                                .await
                                .map_err(err_from!())?;
                        }
                        if let Some(next_tx) = process_tx.orig_tx_id {
                            process_tx = get_transaction(&mut *db_transaction, next_tx)
                                .await
                                .map_err(err_from!())?;
                        } else {
                            break;
                        }
                    }
                    current_tx.orig_tx_id = None;
                    //signed raw data is no longer needed
                    current_tx.signed_raw_data = None;
                    update_tx(&mut *db_transaction, &current_tx)
                        .await
                        .map_err(err_from!())?;
                    db_transaction.commit().await.map_err(err_from!())?;
                    shared_state
                        .lock()
                        .unwrap()
                        .set_tx_message(web3_tx_dao.id, "".to_string());

                    return Ok((current_tx.clone(), ProcessTransactionResult::Confirmed));
                } else {
                    log::info!(
                        "Waiting for confirmations: tx: {}. Current block {}, expected at least: {}",
                        web3_tx_dao.id,
                        current_block_number,
                        block_number + chain_setup.confirmation_blocks
                    );
                }
            } else {
                //add is timed out
                //log::debug!("Receipt not found: {:?}", web3_tx_dao.tx_hash);

                shared_state.lock().unwrap().set_tx_error(
                    web3_tx_dao.id,
                    Some(
                        "Receipt not found despite proper nonce. Probably external payment done."
                            .to_string(),
                    ),
                );

                //if transaction is stuck for more than 5 minutes then remove it from queue
                if let Some(first_stuck_date) = web3_tx_dao.first_stuck_date {
                    let curr_time = chrono::Utc::now();
                    let seconds_elapsed = (curr_time - first_stuck_date).num_seconds();
                    if seconds_elapsed > payment_setup.mark_as_unrecoverable_after_seconds as i64 {
                        if payment_setup.automatic_recover {
                            log::warn!("Recovering from stuck transaction with wrong nonce. In extreme conditions it can lead to double spending, use with caution.");
                            remove_transaction_force(conn, web3_tx_dao.id).await?;
                            return Ok((web3_tx_dao.clone(), ProcessTransactionResult::DoNotSave));
                        }
                        log::error!("Receipt not found despite proper nonce after {} (limit {}). Probably external payment done. Transactions are stuck until you remove transaction from queue.",
                        humantime::format_duration(Duration::from_secs(seconds_elapsed as u64)),
                        humantime::format_duration(Duration::from_secs(payment_setup.mark_as_unrecoverable_after_seconds)));
                        return Ok((
                            web3_tx_dao.clone(),
                            ProcessTransactionResult::NeedRetry("Receipt not found".to_string()),
                        ));
                    } else {
                        log::warn!(
                            "Receipt not found despite proper nonce for {} (limit {})",
                            humantime::format_duration(Duration::from_secs(seconds_elapsed as u64)),
                            humantime::format_duration(Duration::from_secs(
                                payment_setup.mark_as_unrecoverable_after_seconds
                            ))
                        );
                    }
                } else {
                    log::warn!("Receipt not found despite proper nonce. Probably external payment done or web3 provider not yet synchronized");
                    web3_tx_dao.first_stuck_date = Some(chrono::Utc::now());
                    update_tx_stuck_date(conn, web3_tx_dao)
                        .await
                        .map_err(err_from!())?;
                }
            }
        } else {
            log::info!(
                "Waiting until transaction processed on blockchain. Latest nonce too low: {} vs {}",
                latest_nonce,
                transaction_nonce + 1
            );
        }
        log::debug!(
            "Checking pending nonce tx: {}, expected nonce: {}",
            web3_tx_dao.id,
            transaction_nonce + 1
        );
        let pending_nonce = get_transaction_count(from_addr, web3.clone(), true)
            .await
            .map_err(err_from!())?;

        let max_tx_fee_per_gas_str =
            web3_tx_dao
                .max_fee_per_gas
                .clone()
                .ok_or(err_create!(TransactionFailedError::new(
                    "Max fee per gas not found"
                )))?;
        let max_tx_priority_fee_str =
            web3_tx_dao
                .priority_fee
                .clone()
                .ok_or(err_create!(TransactionFailedError::new(
                    "Priority fee not found"
                )))?;
        let tx_fee_per_gas = max_tx_fee_per_gas_str.to_gwei().map_err(err_from!())?;
        let max_fee_per_gas = chain_setup.max_fee_per_gas.to_gwei().map_err(err_from!())?;
        let tx_pr_fee_u256 = max_tx_priority_fee_str.to_u256().map_err(err_from!())?;
        let tx_pr_fee = tx_pr_fee_u256.to_gwei().map_err(err_from!())?;
        let config_priority_fee = chain_setup.priority_fee.to_gwei().map_err(err_from!())?;

        if tx_pr_fee > tx_fee_per_gas {
            log::error!(
                "Transaction priority fee is greater than max fee per gas for tx: {}",
                web3_tx_dao.id
            );
            return Err(err_custom_create!(
                "Transaction priority fee is greater than max fee per gas for tx: {}",
                web3_tx_dao.id
            ));
        }

        let is_ready_for_replacement = if let Some(first_processed) = web3_tx_dao.first_processed {
            let now = chrono::Utc::now();
            let diff = now - first_processed;
            if diff.num_seconds() < -10 {
                log::warn!("Time changed?? time diff lower than 0");
            }
            diff.num_seconds() > chain_setup.replacement_timeout.unwrap_or(60.0).floor() as i64
        } else {
            false
        };

        if is_ready_for_replacement {
            let mut fee_per_gas_changed = false;
            let mut fee_per_gas_bumped_10 = false;
            if tx_fee_per_gas != max_fee_per_gas {
                fee_per_gas_changed = true;
                if tx_fee_per_gas * Decimal::from(11) <= max_fee_per_gas * Decimal::from(10) {
                    fee_per_gas_bumped_10 = true;
                    log::warn!(
                        "Transaction max fee bumped more than 10% from {} to {} for tx: {}",
                        max_tx_fee_per_gas_str,
                        chain_setup.max_fee_per_gas,
                        web3_tx_dao.id
                    );
                } else {
                    log::warn!(
                        "Transaction max fee changed less than 10% more from {} to {} for tx: {}",
                        max_tx_fee_per_gas_str,
                        chain_setup.max_fee_per_gas,
                        web3_tx_dao.id
                    );
                }
            }

            let mut priority_fee_changed = false;
            let mut priority_fee_changed_10 = false;
            if tx_pr_fee != config_priority_fee {
                priority_fee_changed = true;
                if tx_pr_fee * Decimal::from(11) <= config_priority_fee * Decimal::from(10) {
                    priority_fee_changed_10 = true;
                    log::warn!(
                        "Transaction priority fee bumped more than 10% from {} to {} for tx: {}",
                        max_tx_priority_fee_str,
                        chain_setup.priority_fee,
                        web3_tx_dao.id
                    );
                } else {
                    log::warn!(
                        "Transaction priority fee changed less than 10% more from {} to {} for tx: {}",
                        max_tx_priority_fee_str,
                        chain_setup.priority_fee,
                        web3_tx_dao.id
                    );
                }
            }

            if fee_per_gas_changed || priority_fee_changed {
                let mut send_replacement_tx = false;
                let mut replacement_priority_fee = chain_setup.priority_fee;
                let mut replacement_max_fee_per_gas = chain_setup.max_fee_per_gas;
                if priority_fee_changed_10 && fee_per_gas_bumped_10 {
                    send_replacement_tx = true;
                } else if fee_per_gas_bumped_10 && !priority_fee_changed_10 {
                    replacement_priority_fee =
                        tx_pr_fee_u256 * U256::from(11) / U256::from(10) + U256::from(1);

                    if replacement_priority_fee > replacement_max_fee_per_gas {
                        //priority fee cannot be greater than max fee per gas
                        //it should cover very niche case, because priority fee is lower than max fee per gas
                        replacement_max_fee_per_gas = replacement_priority_fee;
                    }
                    log::warn!(
                        "Replacement priority fee is bumped by 10% from {} to {}",
                        tx_pr_fee,
                        replacement_priority_fee.to_gwei().map_err(err_from!())?
                    );
                    send_replacement_tx = true;
                } else {
                    log::warn!("Condition for replacement transactions are not met");
                }

                if send_replacement_tx {
                    let mut tx = web3_tx_dao.clone();
                    let new_tx_dao = TxDbObj {
                        id: 0,
                        method: tx.method.clone(),
                        from_addr: tx.from_addr.clone(),
                        to_addr: tx.to_addr.clone(),
                        chain_id: tx.chain_id,
                        gas_limit: tx.gas_limit,
                        max_fee_per_gas: Some(replacement_max_fee_per_gas.to_string()),
                        priority_fee: Some(replacement_priority_fee.to_string()),
                        val: tx.val.clone(),
                        nonce: tx.nonce,
                        processing: tx.processing,
                        call_data: tx.call_data.clone(),
                        created_date: chrono::Utc::now(),
                        first_processed: None,
                        tx_hash: None,
                        signed_raw_data: None,
                        signed_date: None,
                        broadcast_date: None,
                        broadcast_count: 0,
                        first_stuck_date: None,
                        confirm_date: None,
                        blockchain_date: None,
                        gas_used: None,
                        block_number: None,
                        chain_status: None,
                        block_gas_price: None,
                        effective_gas_price: None,
                        fee_paid: None,
                        error: None,
                        engine_message: None,
                        engine_error: None,
                        orig_tx_id: Some(tx.id),
                    };
                    // used only for specific case testing
                    if let Some(Some(erc20_lib_test_replacement_timeout)) = payment_setup
                        .extra_options_for_testing
                        .as_ref()
                        .map(|testing| testing.erc20_lib_test_replacement_timeout)
                    {
                        log::warn!(
                            "TESTING - sleeping for {} seconds",
                            erc20_lib_test_replacement_timeout.as_secs()
                        );
                        tokio::time::sleep(erc20_lib_test_replacement_timeout).await;
                    }
                    let mut db_transaction = conn.begin().await.map_err(err_from!())?;
                    let new_tx_dao = insert_tx(&mut *db_transaction, &new_tx_dao)
                        .await
                        .map_err(err_from!())?;
                    tx.processing = 0;
                    update_tx(&mut *db_transaction, &tx)
                        .await
                        .map_err(err_from!())?;
                    db_transaction.commit().await.map_err(err_from!())?;
                    log::warn!("Replacement transaction created {}", new_tx_dao.id);

                    return Ok((web3_tx_dao.clone(), ProcessTransactionResult::Replaced));
                }
            }
        }

        if pending_nonce
            <= web3_tx_dao
                .nonce
                .map(|n| n as u64)
                .ok_or_else(|| err_custom_create!("Nonce not found"))?
        {
            // this resend is safe because all tx data is the same,
            // it's just attempt of sending the same transaction
            log::warn!(
                "Resend because pending nonce too low. tx: {} tx_hash: {:?}",
                web3_tx_dao.id,
                web3_tx_dao.tx_hash.clone().unwrap_or_default()
            );
            shared_state
                .lock()
                .unwrap()
                .set_tx_message(web3_tx_dao.id, "Resending transaction".to_string());

            send_transaction(
                conn,
                chain_setup.chain_id,
                chain_setup.glm_address,
                event_sender.clone(),
                web3.clone(),
                web3_tx_dao,
                chain_setup.wrapper_contract_address,
            )
            .await?;
            web3_tx_dao.broadcast_count += 1;
            update_tx(conn, web3_tx_dao).await.map_err(err_from!())?;
            tokio::time::sleep(Duration::from_secs(
                payment_setup.process_interval_after_send,
            ))
            .await;
            continue;
        } else {
            //timeout transaction when it is not confirmed after transaction_timeout seconds
            if let Some(first_processed) = web3_tx_dao.first_processed {
                let now = chrono::Utc::now();
                let diff = now - first_processed;
                if diff.num_seconds() < -10 {
                    log::warn!("Time changed?? time diff lower than 0");
                }
                if diff.num_seconds() > chain_setup.transaction_timeout as i64 {
                    if web3_tx_dao.broadcast_date.is_some() {
                        //if transaction was already broad-casted and still not processed then we can assume that gas price is too low

                        //Check if really gas price is the reason of problems
                        if let Ok(Some(block)) = web3
                            .clone()
                            .eth_block(BlockId::Number(BlockNumber::Latest))
                            .await
                        {
                            let block_base_fee_per_gas_gwei = block
                                .base_fee_per_gas
                                .unwrap_or_default()
                                .to_gwei()
                                .map_err(err_from!())?;

                            let max_tx_fee_per_gas_str =
                                web3_tx_dao.max_fee_per_gas.clone().ok_or(err_create!(
                                    TransactionFailedError::new("Max fee per gas not found")
                                ))?;

                            let tx_max_fee_per_gas_gwei =
                                max_tx_fee_per_gas_str.to_gwei().map_err(err_from!())?;
                            let assumed_min_priority_fee_gwei = if web3_tx_dao.chain_id == 137 {
                                const POLYGON_MIN_PRIORITY_FEE_FOR_GAS_PRICE_CHECK: u32 = 30;
                                Decimal::from(POLYGON_MIN_PRIORITY_FEE_FOR_GAS_PRICE_CHECK)
                            } else if web3_tx_dao.chain_id == 80001 {
                                const MUMBAI_MIN_PRIORITY_FEE_FOR_GAS_PRICE_CHECK: u32 = 1;
                                Decimal::from(MUMBAI_MIN_PRIORITY_FEE_FOR_GAS_PRICE_CHECK)
                            } else {
                                const OTHER_MIN_PRIORITY_FEE_FOR_GAS_PRICE_CHECK: u32 = 0;
                                Decimal::from(OTHER_MIN_PRIORITY_FEE_FOR_GAS_PRICE_CHECK)
                            };

                            if let Some(block_date) = datetime_from_u256_timestamp(block.timestamp)
                            {
                                if block_base_fee_per_gas_gwei + assumed_min_priority_fee_gwei
                                    > tx_max_fee_per_gas_gwei
                                {
                                    send_driver_event(
                                        &event_sender,
                                        DriverEventContent::TransactionStuck(TransactionStuckReason::GasPriceLow(GasLowInfo {
                                            tx: web3_tx_dao.clone(),
                                            tx_max_fee_per_gas_gwei,
                                            block_date,
                                            block_number: block.number.unwrap().as_u64(),
                                            block_base_fee_per_gas_gwei,
                                            assumed_min_priority_fee_gwei,
                                            user_friendly_message: format!(
                                                "Transaction not processed after {} seconds, block base fee per gas + priority fee: {} Gwei is greater than transaction max fee per gas: {} Gwei",
                                                chain_setup.transaction_timeout,
                                                block_base_fee_per_gas_gwei + assumed_min_priority_fee_gwei,
                                                tx_max_fee_per_gas_gwei
                                            ),
                                        })),
                                    )
                                    .await;
                                }
                            }
                        }
                    }

                    log::warn!("Transaction timeout for tx id: {}", web3_tx_dao.id);
                    //return Ok(ProcessTransactionResult::NeedRetry("Timeout".to_string()));
                }
            }
        }
        if !wait_for_confirmation {
            return Ok((web3_tx_dao.clone(), ProcessTransactionResult::Unknown));
        }
        tokio::time::sleep(Duration::from_secs(payment_setup.process_interval)).await;
    }
}
