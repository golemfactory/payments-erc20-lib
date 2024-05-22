use std::str::FromStr;
use std::sync::Arc;

use crate::error::{ErrorBag, PaymentError};
use erc20_payment_lib_common::ops::*;

use crate::transaction::{find_receipt_extended, FindReceiptParseResult};
use crate::utils::{ConversionError, U256ConvExt};

use crate::err_from;
use crate::setup::ChainSetup;

use crate::contracts::encode_erc20_balance_of;
use erc20_payment_lib_common::model::{ChainTxDbObj, TransferInDbObj};
use erc20_rpc_pool::Web3RpcPool;
use sqlx::SqlitePool;
use web3::types::{Address, BlockNumber, CallRequest, U256};

pub async fn add_payment_request_2(
    conn: &SqlitePool,
    token_address: Option<Address>,
    token_amount: U256,
    payment_id: &str,
    payer_addr: Address,
    receiver_addr: Address,
    chain_id: i64,
) -> Result<TransferInDbObj, PaymentError> {
    let transfer_in = TransferInDbObj {
        id: 0,
        payment_id: payment_id.to_string(),
        from_addr: format!("{payer_addr:#x}"),
        receiver_addr: format!("{receiver_addr:#x}"),
        chain_id,
        token_addr: token_address.map(|a| format!("{a:#x}")),
        token_amount: token_amount.to_string(),
        tx_hash: None,
        requested_date: chrono::Utc::now(),
        received_date: None,
    };
    insert_transfer_in(conn, &transfer_in)
        .await
        .map_err(err_from!())
}

pub async fn add_glm_request(
    conn: &SqlitePool,
    chain_setup: &ChainSetup,
    token_amount: U256,
    payment_id: &str,
    payer_addr: Address,
    receiver_addr: Address,
) -> Result<TransferInDbObj, PaymentError> {
    let transfer_in = TransferInDbObj {
        id: 0,
        payment_id: payment_id.to_string(),
        from_addr: format!("{payer_addr:#x}"),
        receiver_addr: format!("{receiver_addr:#x}"),
        chain_id: chain_setup.chain_id,
        token_addr: Some(format!("{:#x}", chain_setup.glm_address)),
        token_amount: token_amount.to_string(),
        tx_hash: None,
        requested_date: chrono::Utc::now(),
        received_date: None,
    };
    insert_transfer_in(conn, &transfer_in)
        .await
        .map_err(err_from!())
}

pub async fn transaction_from_chain_and_into_db(
    web3: Arc<Web3RpcPool>,
    conn: &SqlitePool,
    chain_id: i64,
    tx_hash: &str,
    glm_address: Address,
    get_balances: bool,
) -> Result<Option<ChainTxDbObj>, PaymentError> {
    log::debug!("tx_hash: {tx_hash}");
    let tx_hash = web3::types::H256::from_str(tx_hash)
        .map_err(|_err| ConversionError::from("Cannot parse tx_hash".to_string()))
        .map_err(err_from!())?;

    if let Some(chain_tx) = get_chain_tx_hash(conn, format!("{:#x}", tx_hash))
        .await
        .map_err(err_from!())?
    {
        log::warn!("Transaction already in DB: {}, skipping...", chain_tx.id);
        return Ok(Some(chain_tx));
    }

    let (mut chain_tx_dao, transfers) =
        match find_receipt_extended(web3.clone(), tx_hash, chain_id, glm_address).await? {
            FindReceiptParseResult::Success((c, t)) => (c, t),
            FindReceiptParseResult::Failure(str) => {
                log::warn!("Transaction cannot be parsed: {}", str);
                return Ok(None);
            }
        };

    if chain_tx_dao.chain_status != 1 {
        return Ok(None);
    }

    if get_balances {
        let mut loop_no = 0;

        let balance = loop {
            loop_no += 1;
            match web3
                .clone()
                .eth_balance(
                    Address::from_str(&chain_tx_dao.from_addr).unwrap(),
                    Some(BlockNumber::Number(chain_tx_dao.block_number.into())),
                )
                .await
            {
                Ok(v) => break Some(v),
                Err(e) => {
                    log::debug!("Error getting balance: {}", e);
                    if loop_no > 1000 {
                        break None;
                    }
                }
            };
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        };

        log::debug!(
            "Balance: {:.5} for block {}",
            balance.unwrap_or_default().to_eth().unwrap(),
            chain_tx_dao.block_number
        );

        loop_no = 0;
        let token_balance = loop {
            let call_data =
                encode_erc20_balance_of(Address::from_str(&chain_tx_dao.from_addr).unwrap())
                    .map_err(err_from!())?;
            match web3
                .clone()
                .eth_call(
                    CallRequest {
                        to: Some(glm_address),
                        data: Some(web3::types::Bytes::from(call_data)),
                        ..Default::default()
                    },
                    Some(web3::types::BlockId::Number(BlockNumber::Number(
                        chain_tx_dao.block_number.into(),
                    ))),
                )
                .await
            {
                Ok(v) => {
                    if v.0.len() == 32 {
                        break Some(U256::from_big_endian(&v.0));
                    }
                }
                Err(e) => {
                    log::debug!("Error getting token balance: {}", e);
                }
            };
            if loop_no > 1000 {
                break None;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        };

        log::info!(
            "Token balance: {:.5} for block {}",
            token_balance
                .map(|v| v.to_eth().unwrap())
                .unwrap_or_default(),
            chain_tx_dao.block_number
        );

        chain_tx_dao.balance_eth = balance.map(|b| b.to_string());
        chain_tx_dao.balance_glm = token_balance.map(|v| v.to_string());
    }

    let mut db_transaction = conn.begin().await.map_err(err_from!())?;

    let tx = insert_chain_tx(&mut *db_transaction, &chain_tx_dao)
        .await
        .map_err(err_from!())?;

    if !transfers.is_empty() {
        //This is a bit complicated, but we need to distribute the fee paid by the user in transaction
        //to all token transfers in the transaction in the way that sum of fees is correct
        //Implementation is a bit rough, but it works
        let mut distribute_fee: Vec<Option<U256>> = Vec::with_capacity(transfers.len());
        let val = U256::from_dec_str(&tx.fee_paid)
            .map_err(|_err| ConversionError::from("failed to parse fee paid".into()))
            .map_err(err_from!())?;
        let mut fee_left = val;
        let val_share = val / U256::from(transfers.len() as u64);
        for _tt in &transfers {
            fee_left -= val_share;
            distribute_fee.push(Some(val_share));
        }
        let fee_left = fee_left.as_u64() as usize;
        if fee_left >= transfers.len() {
            panic!(
                "fee left is too big, critical error when distributing fee {}/{}",
                fee_left,
                transfers.len()
            );
        }
        //distribute the rest of the fee by adding one am much time as needed
        distribute_fee.iter_mut().take(fee_left).for_each(|item| {
            let val = item.unwrap();
            *item = Some(val + U256::from(1));
        });

        for (mut transfer, fee_paid) in transfers.into_iter().zip(distribute_fee) {
            transfer.chain_tx_id = tx.id;
            transfer.fee_paid = fee_paid.map(|v| v.to_string());
            insert_chain_transfer(&mut *db_transaction, &transfer)
                .await
                .map_err(err_from!())?;
        }
    }

    db_transaction.commit().await.map_err(err_from!())?;
    log::debug!("Transaction found and parsed successfully: {}", tx.id);
    Ok(Some(tx))
}

/*
pub async fn confirm_loop(
    _shared_state: Arc<Mutex<SharedState>>,
    _conn: &SqlitePool,
    payment_setup: &PaymentSetup,
) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(
            payment_setup.process_interval,
        ))
        .await;
    }
}*/
