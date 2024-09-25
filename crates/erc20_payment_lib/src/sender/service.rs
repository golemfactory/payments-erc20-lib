use crate::error::{ErrorBag, PaymentError};
use erc20_payment_lib_common::ops::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

use crate::sender::process::{process_transaction, ProcessTransactionResult};

use crate::utils::ConversionError;

use crate::runtime::{send_driver_event, SharedState};
use crate::sender::batching::{gather_transactions_post, gather_transactions_pre};
use crate::sender::process_allowance;
use crate::setup::PaymentSetup;
use crate::signer::{Signer, SignerAccount};
use crate::{err_create, err_custom_create, err_from};
use erc20_payment_lib_common::model::TxDbObj;
use erc20_payment_lib_common::{DriverEvent, DriverEventContent, TransactionFinishedInfo};
use sqlx::SqlitePool;
use tokio::select;
use tokio::time::Instant;
use web3::types::{Address, U256};

pub async fn update_token_transfer_result(
    event_sender: Option<tokio::sync::mpsc::Sender<DriverEvent>>,
    conn: &SqlitePool,
    tx: &mut TxDbObj,
    process_t_res: &ProcessTransactionResult,
) -> Result<(), PaymentError> {
    match process_t_res {
        ProcessTransactionResult::Confirmed => {
            tx.processing = 0;

            let mut db_transaction = conn.begin().await.map_err(err_from!())?;
            let mut token_transfers = get_token_transfers_by_tx(&mut *db_transaction, tx.id)
                .await
                .map_err(err_from!())?;

            if token_transfers.is_empty() {
                log::error!("Transaction {} has no token transfers", tx.id);
                return Err(err_custom_create!(
                    "Transaction has no attached token transfers in db {}",
                    tx.id
                ));
            }

            //This is a bit complicated, but we need to distribute the fee paid by the user in transaction
            //to all token transfers in the transaction in the way that sum of fees is correct
            //Implementation is a bit rough, but it works
            let mut distribute_fee: Vec<Option<U256>> = Vec::with_capacity(token_transfers.len());
            if let Some(fee_paid) = tx.fee_paid.clone() {
                let val = U256::from_dec_str(&fee_paid)
                    .map_err(|_err| ConversionError::from("failed to parse fee paid".into()))
                    .map_err(err_from!())?;
                let mut fee_left = val;
                let val_share = val / U256::from(token_transfers.len() as u64);
                for _tt in &token_transfers {
                    fee_left -= val_share;
                    distribute_fee.push(Some(val_share));
                }
                let fee_left = fee_left.as_u64() as usize;
                if fee_left >= token_transfers.len() {
                    panic!(
                        "fee left is too big, critical error when distributing fee {}/{}",
                        fee_left,
                        token_transfers.len()
                    );
                }
                //distribute the rest of the fee by adding one am much time as needed
                distribute_fee.iter_mut().take(fee_left).for_each(|item| {
                    let val = item.unwrap();
                    *item = Some(val + U256::from(1));
                });
            } else {
                for _tt in &token_transfers {
                    distribute_fee.push(None);
                }
            }

            for (token_transfer, fee_paid) in token_transfers.iter_mut().zip(distribute_fee) {
                token_transfer.fee_paid = fee_paid.map(|v| v.to_string());
                token_transfer.paid_date = Some(chrono::Utc::now());

                update_token_transfer(&mut *db_transaction, token_transfer)
                    .await
                    .map_err(err_from!())?;
            }
            update_tx(&mut *db_transaction, tx)
                .await
                .map_err(err_from!())?;
            db_transaction.commit().await.map_err(err_from!())?;
            //if transaction is committed emit events:
            for token_transfer in token_transfers {
                send_driver_event(
                    &event_sender,
                    DriverEventContent::TransferFinished(TransactionFinishedInfo {
                        token_transfer_dao: token_transfer,
                        tx_dao: tx.clone(),
                    }),
                )
                .await;
            }
        }
        ProcessTransactionResult::NeedRetry(err) => {
            tx.processing = 0;

            let mut db_transaction = conn.begin().await.map_err(err_from!())?;
            let token_transfers = get_token_transfers_by_tx(&mut *db_transaction, tx.id)
                .await
                .map_err(err_from!())?;
            for mut token_transfer in token_transfers {
                token_transfer.fee_paid = Some("0".to_string());
                token_transfer.error = Some(err.clone());
                update_token_transfer(&mut *db_transaction, &token_transfer)
                    .await
                    .map_err(err_from!())?;
            }
            tx.error = Some(err.clone());

            update_tx(&mut *db_transaction, tx)
                .await
                .map_err(err_from!())?;
            db_transaction.commit().await.map_err(err_from!())?;
        }
        ProcessTransactionResult::InternalError(err) => {
            tx.processing = 0;

            let mut db_transaction = conn.begin().await.map_err(err_from!())?;
            let token_transfers = get_token_transfers_by_tx(&mut *db_transaction, tx.id)
                .await
                .map_err(err_from!())?;
            for mut token_transfer in token_transfers {
                token_transfer.fee_paid = Some("0".to_string());
                token_transfer.error = Some(err.clone());
                update_token_transfer(&mut *db_transaction, &token_transfer)
                    .await
                    .map_err(err_from!())?;
            }
            tx.error = Some(err.clone());

            update_tx(&mut *db_transaction, tx)
                .await
                .map_err(err_from!())?;
            db_transaction.commit().await.map_err(err_from!())?;
        }
        ProcessTransactionResult::Unknown => {
            tx.processing = 1;
            update_tx(conn, tx).await.map_err(err_from!())?;
        }
        _ => {}
    }
    Ok(())
}

pub async fn update_approve_result(
    event_sender: Option<tokio::sync::mpsc::Sender<DriverEvent>>,
    conn: &SqlitePool,
    tx: &mut TxDbObj,
    process_t_res: &ProcessTransactionResult,
) -> Result<(), PaymentError> {
    match process_t_res {
        ProcessTransactionResult::Confirmed => {
            tx.processing = 0;

            let mut db_transaction = conn.begin().await.map_err(err_from!())?;
            let mut allowance = get_allowance_by_tx(&mut *db_transaction, tx.id)
                .await
                .map_err(err_from!())?;
            allowance.fee_paid.clone_from(&tx.fee_paid);
            update_allowance(&mut *db_transaction, &allowance)
                .await
                .map_err(err_from!())?;
            update_tx(&mut *db_transaction, tx)
                .await
                .map_err(err_from!())?;
            db_transaction.commit().await.map_err(err_from!())?;
            //if transaction is committed emit events:
            send_driver_event(
                &event_sender,
                DriverEventContent::ApproveFinished(allowance),
            )
            .await;
        }
        ProcessTransactionResult::NeedRetry(err) => {
            tx.processing = 0;
            let mut db_transaction = conn.begin().await.map_err(err_from!())?;
            let mut allowance = get_allowance_by_tx(&mut *db_transaction, tx.id)
                .await
                .map_err(err_from!())?;
            allowance.fee_paid = Some("0".to_string());
            allowance.error = Some(err.clone());
            tx.error = Some(err.clone());
            update_allowance(&mut *db_transaction, &allowance)
                .await
                .map_err(err_from!())?;
            update_tx(&mut *db_transaction, tx)
                .await
                .map_err(err_from!())?;
            db_transaction.commit().await.map_err(err_from!())?;
        }
        ProcessTransactionResult::InternalError(err) => {
            tx.processing = 0;
            let mut db_transaction = conn.begin().await.map_err(err_from!())?;
            let mut allowance = get_allowance_by_tx(&mut *db_transaction, tx.id)
                .await
                .map_err(err_from!())?;
            allowance.fee_paid = Some("0".to_string());
            allowance.error = Some(err.clone());
            tx.error = Some(err.clone());
            update_allowance(&mut *db_transaction, &allowance)
                .await
                .map_err(err_from!())?;
            update_tx(&mut *db_transaction, tx)
                .await
                .map_err(err_from!())?;
            db_transaction.commit().await.map_err(err_from!())?;
        }
        ProcessTransactionResult::Unknown => {
            tx.processing = 1;
            update_tx(conn, tx).await.map_err(err_from!())?;
        }
        _ => {}
    }
    Ok(())
}

pub async fn update_tx_result(
    conn: &SqlitePool,
    tx: &mut TxDbObj,
    process_t_res: &ProcessTransactionResult,
) -> Result<(), PaymentError> {
    match process_t_res {
        ProcessTransactionResult::Confirmed => {
            tx.processing = 0;
            update_tx(conn, tx).await.map_err(err_from!())?;
        }
        ProcessTransactionResult::NeedRetry(_err) => {
            tx.processing = 0;
            tx.error = Some("Need retry".to_string());
            update_tx(conn, tx).await.map_err(err_from!())?;
        }
        ProcessTransactionResult::InternalError(err) => {
            tx.processing = 0;
            tx.error = Some(err.clone());
            update_tx(conn, tx).await.map_err(err_from!())?;
        }
        ProcessTransactionResult::Unknown => {
            tx.processing = 1;
            update_tx(conn, tx).await.map_err(err_from!())?;
        }
        _ => {}
    }
    Ok(())
}

pub async fn process_transactions(
    signer_account: &SignerAccount,
    chain_id: i64,
    event_sender: Option<tokio::sync::mpsc::Sender<DriverEvent>>,
    shared_state: Arc<std::sync::Mutex<SharedState>>,
    conn: &SqlitePool,
    payment_setup: &PaymentSetup,
    signer: Arc<Box<dyn Signer + Send + Sync + 'static>>,
) -> Result<(), PaymentError> {
    //remove tx from current processing infos

    let mut current_wait_time_no_gas_token: f64 = 0.0;
    loop {
        let mut transactions =
            get_next_transactions_to_process(conn, Some(signer_account.address), 1, chain_id)
                .await
                .map_err(err_from!())?;

        let Some(tx) = transactions.get_mut(0) else {
            log::debug!("No transactions to process, breaking from loop");
            break;
        };

        let (mut tx, process_t_res) = if shared_state.lock().unwrap().is_skipped(tx.id) {
            (
                tx.clone(),
                ProcessTransactionResult::InternalError("Transaction skipped by user".into()),
            )
        } else {
            shared_state
                .lock()
                .unwrap()
                .set_tx_message(tx.id, "Processing".to_string());
            match process_transaction(
                event_sender.clone(),
                shared_state.clone(),
                conn,
                tx,
                payment_setup,
                signer.clone(),
                false,
            )
            .await
            {
                Ok((tx_dao, process_result)) => (tx_dao, process_result),
                Err(err) => match err.inner {
                    ErrorBag::TransactionFailedError(err2) => {
                        shared_state
                            .lock()
                            .unwrap()
                            .set_tx_error(tx.id, Some(err2.message.clone()));

                        return Err(err_create!(err2));
                    }
                    _ => {
                        log::error!("Error in process transaction: {}", err.inner);
                        shared_state
                            .lock()
                            .unwrap()
                            .set_tx_error(tx.id, Some(format!("{}", err.inner)));
                        return Err(err);
                    }
                },
            }
        };
        if let ProcessTransactionResult::DoNotSaveWaitForGasOrToken = process_t_res {
            //pass
        } else {
            //clear wait flag if other result encountered
            current_wait_time_no_gas_token = 0.0;
        }
        if let ProcessTransactionResult::Replaced = process_t_res {
            shared_state.lock().unwrap().current_tx_info.remove(&tx.id);
            continue;
        };

        #[allow(clippy::if_same_then_else)]
        if tx.method.starts_with("MULTI.golemTransfer")
            || tx.method == "ERC20.transfer"
            || tx.method == "transfer"
        {
            log::debug!("Updating token transfer result");
            update_token_transfer_result(event_sender.clone(), conn, &mut tx, &process_t_res)
                .await?;
        } else if tx.method == "LOCK.depositSingleTransfer" || tx.method == "LOCK.depositTransfer" {
            log::debug!("Updating token transfer result");
            update_token_transfer_result(event_sender.clone(), conn, &mut tx, &process_t_res)
                .await?;
        } else if tx.method == "LOCK.depositSingleTransferAndClose"
            || tx.method == "LOCK.depositTransferAndClose"
        {
            log::debug!("Updating token transfer result");
            update_token_transfer_result(event_sender.clone(), conn, &mut tx, &process_t_res)
                .await?;
        } else if tx.method == "ERC20.approve" {
            log::debug!("Updating token approve result");
            update_approve_result(event_sender.clone(), conn, &mut tx, &process_t_res).await?;
        } else {
            log::debug!("Updating plain tx result");
            update_tx_result(conn, &mut tx, &process_t_res).await?;
        }
        match process_t_res {
            ProcessTransactionResult::Unknown => {}
            ProcessTransactionResult::Confirmed => {
                send_driver_event(
                    &event_sender,
                    DriverEventContent::TransactionConfirmed(tx.clone()),
                )
                .await;
                //proces next transaction without waiting
                continue;
            }
            _ => {
                shared_state.lock().unwrap().current_tx_info.remove(&tx.id);
            }
        }
        if let ProcessTransactionResult::DoNotSaveWaitForGasOrToken = process_t_res {
            //we need to wait for gas or token
            if current_wait_time_no_gas_token
                < payment_setup.process_interval_after_no_gas_or_token_start as f64
            {
                current_wait_time_no_gas_token =
                    payment_setup.process_interval_after_no_gas_or_token_start as f64;
            } else {
                current_wait_time_no_gas_token *=
                    payment_setup.process_interval_after_no_gas_or_token_increase;
            }
            if current_wait_time_no_gas_token
                >= payment_setup.process_interval_after_no_gas_or_token_max as f64
            {
                current_wait_time_no_gas_token =
                    payment_setup.process_interval_after_no_gas_or_token_max as f64;
            }
            log::warn!(
                "Sleeping for {:.2} seconds (sleep after no gas or token)",
                current_wait_time_no_gas_token
            );

            tokio::time::sleep(Duration::from_secs_f64(current_wait_time_no_gas_token)).await;
        } else {
            log::debug!(
                "Sleeping for {} seconds (process interval)",
                payment_setup.process_interval
            );
            tokio::time::sleep(Duration::from_secs(payment_setup.process_interval)).await;
        }
    }
    Ok(())
}

fn get_next_gather_time(
    account: &SignerAccount,
    last_gather_time: chrono::DateTime<chrono::Utc>,
    gather_transactions_interval: i64,
) -> chrono::DateTime<chrono::Utc> {
    let next_gather_time = last_gather_time
        + chrono::Duration::try_seconds(gather_transactions_interval)
            .expect("Invalid gather interval");

    if let Some(external_gather_time) = *account.external_gather_time.lock().unwrap() {
        std::cmp::min(external_gather_time, next_gather_time)
    } else {
        next_gather_time
    }
}

fn get_next_gather_time_and_clear_if_success(
    account: &SignerAccount,
    last_gather_time: chrono::DateTime<chrono::Utc>,
    gather_transactions_interval: i64,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let mut external_gather_time_guard = account.external_gather_time.lock().unwrap();

    let next_gather_time = last_gather_time
        + chrono::Duration::try_seconds(gather_transactions_interval)
            .expect("Invalid gather transactions interval");

    let next_gather_time = if let Some(external_gather_time) = *external_gather_time_guard {
        std::cmp::min(external_gather_time, next_gather_time)
    } else {
        next_gather_time
    };

    if chrono::Utc::now() >= next_gather_time {
        *external_gather_time_guard = None;
        return None;
    }
    Some(next_gather_time)
}

async fn sleep_for_gather_time_or_report_alive(
    account: &SignerAccount,
    wake: Arc<Notify>,
    last_gather_time: chrono::DateTime<chrono::Utc>,
    payment_setup: PaymentSetup,
) {
    let gather_transactions_interval = payment_setup.gather_interval as i64;
    let started_sleep = chrono::Utc::now();
    loop {
        let current_time = chrono::Utc::now();
        let already_slept = current_time - started_sleep;
        let next_gather_time =
            get_next_gather_time(account, last_gather_time, gather_transactions_interval);
        if current_time >= next_gather_time {
            break;
        }
        let max_sleep_time = payment_setup.report_alive_interval as f64
            - already_slept.num_milliseconds() as f64 / 1000.0;
        if max_sleep_time <= 0.0 {
            break;
        }
        let sleep_time = Duration::from_secs_f64(
            max_sleep_time
                .min((next_gather_time - current_time).num_milliseconds() as f64 / 1000.0),
        );
        select! {
            _ = tokio::time::sleep(sleep_time) => {
                log::debug!("Finished sleeping");
                break;
            }
            _ = wake.notified() => {
                log::debug!("Woken up by external event");
            }
        }
    }
}

pub async fn service_loop(
    shared_state: Arc<std::sync::Mutex<SharedState>>,
    chain_id: i64,
    account: Address,
    wake: Arc<tokio::sync::Notify>,
    conn: &SqlitePool,
    payment_setup: &PaymentSetup,
    event_sender: Option<tokio::sync::mpsc::Sender<DriverEvent>>,
) {
    let gather_transactions_interval = payment_setup.gather_interval as i64;
    let mut last_gather_time = if payment_setup.gather_at_start {
        chrono::Utc::now()
            - chrono::Duration::try_seconds(gather_transactions_interval)
                .expect("Invalid gather interval")
    } else {
        chrono::Utc::now()
    };

    let metric_label_start = "erc20_payment_lib.service_loop.start";
    let metric_label_process_allowance = "erc20_payment_lib.service_loop.process_allowance";
    let metric_label_gather_pre = "erc20_payment_lib.service_loop.gather_pre";
    let metric_label_gather_pre_error = "erc20_payment_lib.service_loop.gather_pre_error";
    let metric_label_gather_post = "erc20_payment_lib.service_loop.gather_post";
    let metric_label_gather_post_error = "erc20_payment_lib.service_loop.gather_post_error";
    //let metric_label_loop_duration = "erc20_payment_lib.service_loop.loop_duration";
    metrics::counter!(metric_label_start, 0, "chain_id" => chain_id.to_string());
    metrics::counter!(metric_label_process_allowance, 0, "chain_id" => chain_id.to_string());
    metrics::counter!(metric_label_gather_pre, 0, "chain_id" => chain_id.to_string());
    metrics::counter!(metric_label_gather_pre_error, 0, "chain_id" => chain_id.to_string());
    metrics::counter!(metric_label_gather_post, 0, "chain_id" => chain_id.to_string());
    metrics::counter!(metric_label_gather_post_error, 0, "chain_id" => chain_id.to_string());

    let mut process_tx_needed;
    let mut last_stats_time: Option<Instant> = None;
    loop {
        log::debug!(
            "Sender service loop - start loop chain id: {} - account: {:#x}",
            chain_id,
            account
        );
        metrics::counter!(metric_label_start, 1);
        let signer_account = match shared_state
            .lock()
            .unwrap()
            .accounts
            .iter()
            .find(|acc| acc.address == account)
        {
            Some(acc) => acc.clone(),
            None => {
                log::warn!("Account {:#x} not found in accounts, exiting...", account);
                break;
            }
        };

        let current_time = chrono::Utc::now();
        let current_time_inst = Instant::now();
        if let Some(_last_stats_time) = last_stats_time {
            //todo - maybe add some metric here if possible
            //metrics::timing!(metric_label_loop_duration, last_stats_time, current_time_inst);
        }
        last_stats_time = Some(current_time_inst);

        if payment_setup.generate_tx_only {
            log::warn!("Skipping processing transactions...");
        } else if let Err(e) = process_transactions(
            &signer_account,
            chain_id,
            event_sender.clone(),
            shared_state.clone(),
            conn,
            payment_setup,
            signer_account.signer.clone(),
        )
        .await
        {
            log::error!("Error in process transactions: {}", e);
            tokio::time::sleep(Duration::from_secs(payment_setup.process_interval)).await;
            continue;
        }

        process_tx_needed = false;

        //we should be here only when all pending transactions are processed

        let next_gather_time = get_next_gather_time_and_clear_if_success(
            &signer_account,
            last_gather_time,
            gather_transactions_interval,
        );

        if !payment_setup.finish_when_done {
            if let Some(next_gather_time) = next_gather_time {
                log::debug!(
                    "Payments will be gathered in {}",
                    humantime::format_duration(Duration::from_secs(
                        (next_gather_time - current_time)
                            .num_seconds()
                            .try_into()
                            .unwrap_or(0)
                    ))
                );
                sleep_for_gather_time_or_report_alive(
                    &signer_account,
                    wake.clone(),
                    last_gather_time,
                    payment_setup.clone(),
                )
                .await;
                continue;
            }
        }

        metrics::counter!(metric_label_gather_pre, 1);

        log::debug!("Gathering payments...");

        let mut token_transfer_map = match gather_transactions_pre(
            &signer_account,
            chain_id,
            conn,
            payment_setup,
            &mut process_tx_needed,
        )
        .await
        {
            Ok(token_transfer_map) => token_transfer_map,
            Err(e) => {
                metrics::counter!(metric_label_gather_pre_error, 1);
                log::error!(
                    "Error in gather transactions, driver will be stuck, Fix DB to continue {:?}",
                    e
                );
                tokio::time::sleep(std::time::Duration::from_secs(
                    payment_setup.process_interval_after_error,
                ))
                .await;
                continue;
            }
        };
        metrics::counter!(metric_label_gather_post, 1);

        match gather_transactions_post(
            event_sender.clone(),
            conn,
            payment_setup,
            &mut token_transfer_map,
        )
        .await
        {
            Ok(count) => {
                if count > 0 {
                    process_tx_needed = true;
                }
            }
            Err(e) => {
                match &e.inner {
                    ErrorBag::NoAllowanceFound(allowance_request) => {
                        log::info!(
                            "No allowance found for contract {} to spend token {} for owner: {}",
                            allowance_request.spender_addr,
                            allowance_request.token_addr,
                            allowance_request.owner
                        );
                        metrics::counter!(metric_label_process_allowance, 1);

                        match process_allowance(
                            conn,
                            payment_setup,
                            allowance_request,
                            signer_account.signer.clone(),
                            event_sender.as_ref(),
                        )
                        .await
                        {
                            Ok(_) => {
                                //process transaction instantly
                                shared_state.lock().unwrap().idling = false;
                                continue;
                            }
                            Err(e) => {
                                log::error!("Error in process allowance: {}", e);
                                tokio::time::sleep(std::time::Duration::from_secs(
                                    payment_setup.process_interval_after_error,
                                ))
                                .await;
                            }
                        }
                    }
                    _ => {
                        log::error!("Error in gather transactions: {}", e);
                    }
                }
                //if error happened, we should check if partial transfers were inserted
                metrics::counter!(metric_label_gather_post_error, 1);
                process_tx_needed = true;
                log::error!("Error in gather transactions: {}", e);
            }
        };
        last_gather_time = current_time;
        if payment_setup.finish_when_done && !process_tx_needed {
            log::info!("No more work to do, exiting...");
            break;
        }
        if !process_tx_needed {
            log::debug!("No work found for now...");
            shared_state.lock().unwrap().idling = true;
        } else {
            shared_state.lock().unwrap().idling = false;
        }
    }
    log::info!("Sender service loop - end loop for account {:#x}", account);
}
