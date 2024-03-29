use crate::options::ScanBlockchainOptions;
use erc20_payment_lib::config::{Chain, Config};
use erc20_payment_lib::service::transaction_from_chain_and_into_db;
use erc20_payment_lib::setup::PaymentSetup;
use erc20_payment_lib::transaction::{import_erc20_txs, ImportErc20TxsArgs};
use erc20_payment_lib_common::error::ErrorBag;
use erc20_payment_lib_common::error::PaymentError;
use erc20_payment_lib_common::model::ScanDaoDbObj;
use erc20_payment_lib_common::ops::{delete_scan_info, get_scan_info, upsert_scan_info};
use erc20_payment_lib_common::{err_custom_create, err_from};
use erc20_rpc_pool::Web3RpcPool;
use sqlx::SqlitePool;
use std::str::FromStr;
use std::sync::Arc;
use web3::types::Address;

async fn scan_int(
    conn: SqlitePool,
    scan_blockchain_options: &ScanBlockchainOptions,
    chain_cfg: Chain,
    web3: Arc<Web3RpcPool>,
    start_block: i64,
    end_block: i64,
    sender: Option<Address>,
) -> Result<(), PaymentError> {
    let txs = import_erc20_txs(ImportErc20TxsArgs {
        web3: web3.clone(),
        erc20_address: chain_cfg.token.address,
        chain_id: chain_cfg.chain_id,
        filter_by_senders: sender.map(|sender| [sender].to_vec()),
        filter_by_receivers: None,
        start_block,
        scan_end_block: end_block,
        blocks_at_once: scan_blockchain_options.blocks_at_once,
    })
    .await
    .map_err(|e| {
        log::error!("Error when importing txs: {}", e);
        e
    })?;

    let mut max_block_from_tx = None;
    for tx in &txs {
        match transaction_from_chain_and_into_db(
            web3.clone(),
            &conn.clone(),
            chain_cfg.chain_id,
            &format!("{tx:#x}"),
            chain_cfg.token.address,
            scan_blockchain_options.import_balances,
        )
        .await
        {
            Ok(Some(chain_tx)) => {
                if chain_tx.block_number > max_block_from_tx.unwrap_or(0) {
                    max_block_from_tx = Some(chain_tx.block_number);
                }
            }
            Ok(None) => {}
            Err(e) => {
                log::error!("Error when getting transaction from chain: {}", e);
                continue;
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn scan_auto_step(
    conn: SqlitePool,
    scan_blockchain_options: &ScanBlockchainOptions,
    chain_cfg: Chain,
    web3: Arc<Web3RpcPool>,
    sender: Option<Address>,
    scan_info: &mut ScanDaoDbObj,
) -> Result<bool, PaymentError> {
    let current_block = web3
        .clone()
        .eth_block_number()
        .await
        .map_err(err_from!())?
        .as_u64() as i64;

    let scan_behind_needed =
        scan_info.start_block < scan_blockchain_options.from_block.unwrap_or(1) as i64;

    let (start_block, end_block, is_forward) = if scan_behind_needed {
        if current_block - scan_info.last_block
            >= scan_blockchain_options.blocks_behind.unwrap_or(100) as i64
                + scan_blockchain_options.forward_scan_buffer as i64
        {
            log::info!("Scan forward needed");
            let start_block = scan_info.last_block + 1;
            if start_block > current_block {
                log::warn!(
                    "Start block {} is higher than current block {}, no newer data on blockchain",
                    start_block,
                    current_block
                );
                return Ok(true);
            }
            let end_block = start_block + scan_blockchain_options.blocks_at_once as i64;
            let end_block = std::cmp::min(end_block, current_block);
            (start_block, end_block, true)
        } else {
            let end_block = scan_info.start_block;
            let start_block =
                std::cmp::max(end_block - scan_blockchain_options.blocks_at_once as i64, 1);
            if end_block - start_block > 0 {
                (start_block, end_block, false)
            } else {
                log::warn!(
                    "Start block {} is higher than end block {}, no newer data on blockchain",
                    start_block,
                    end_block
                );
                return Ok(true);
            }
        }
    } else {
        // normal auto scan
        let start_block = scan_info.last_block - 100;
        if start_block > current_block {
            log::warn!(
                "Start block {} is higher than current block {}, no newer data on blockchain",
                start_block,
                current_block
            );
            return Ok(true);
        }
        let end_block = start_block + scan_blockchain_options.blocks_at_once as i64;
        let end_block = std::cmp::min(end_block, current_block + 1);
        (start_block, end_block, true)
    };

    log::info!(
        "Scanning from {} to {} - direction {}",
        start_block,
        end_block,
        if is_forward { "forward" } else { "backward" }
    );

    scan_int(
        conn.clone(),
        scan_blockchain_options,
        chain_cfg.clone(),
        web3.clone(),
        start_block,
        end_block,
        sender,
    )
    .await?;

    if scan_info.start_block == -1 {
        scan_info.start_block = start_block;
    }

    if is_forward {
        //last blocks may be missing so we subtract 100 blocks from current to be sure
        scan_info.last_block = std::cmp::min(
            end_block,
            current_block - scan_blockchain_options.blocks_behind.unwrap_or(100) as i64,
        );
        log::debug!(
            "Updating db scan entry {} - {}",
            scan_info.start_block,
            scan_info.last_block
        );
    } else {
        scan_info.start_block = start_block;
        log::debug!(
            "Updating db scan entry {} - {}",
            scan_info.start_block,
            scan_info.last_block
        );
    }

    upsert_scan_info(&conn.clone(), scan_info)
        .await
        .map_err(err_from!())?;

    Ok(is_forward)
}

pub async fn scan_blockchain_local(
    conn: SqlitePool,
    scan_blockchain_options: ScanBlockchainOptions,
    config: Config,
) -> Result<(), PaymentError> {
    log::info!("Scanning blockchain {}", scan_blockchain_options.chain_name);

    let payment_setup = PaymentSetup::new_empty(&config)?;
    let chain_cfg = config
        .chain
        .get(&scan_blockchain_options.chain_name)
        .ok_or(err_custom_create!(
            "Chain {} not found in config file",
            scan_blockchain_options.chain_name
        ))?;
    let web3 = payment_setup.get_provider(chain_cfg.chain_id)?;

    let sender = scan_blockchain_options
        .sender
        .clone()
        .map(|s| Address::from_str(&s).unwrap());

    let scan_info = ScanDaoDbObj {
        id: 0,
        chain_id: chain_cfg.chain_id,
        filter: sender
            .map(|f| format!("{f:#x}"))
            .unwrap_or("all".to_string()),
        start_block: -1,
        last_block: -1,
    };
    let scan_info_from_db = get_scan_info(&conn.clone(), chain_cfg.chain_id, &scan_info.filter)
        .await
        .map_err(err_from!())?;

    let scan_info = if scan_blockchain_options.start_new_scan {
        log::warn!("Starting new scan - removing old scan info from db");
        delete_scan_info(&conn.clone(), scan_info.chain_id, &scan_info.filter)
            .await
            .map_err(err_from!())?;
        scan_info
    } else if let Some(scan_info_from_db) = scan_info_from_db {
        log::debug!("Found scan info from db: {:?}", scan_info_from_db);
        scan_info_from_db
    } else {
        scan_info
    };

    println!("scan_info: {:?}", scan_info);

    let current_block = web3
        .clone()
        .eth_block_number()
        .await
        .map_err(err_from!())?
        .as_u64() as i64;

    let from_block: i64 = scan_blockchain_options
        .from_block
        .map(|f| f as i64)
        .unwrap_or(current_block - 100);
    let mut start_block = std::cmp::max(1, from_block);

    if let Some(from_block) = scan_blockchain_options.from_block {
        if from_block > current_block as u64 {
            log::warn!(
                "From block {} is higher than current block {}, no newer data on blockchain",
                from_block,
                current_block
            );
            return Ok(());
        }
    }

    if scan_blockchain_options.auto {
        let mut scan_info = scan_info.clone();
        loop {
            match scan_auto_step(
                conn.clone(),
                &scan_blockchain_options,
                chain_cfg.clone(),
                web3.clone(),
                sender,
                &mut scan_info,
            )
            .await
            {
                Ok(wait) => {
                    log::info!("Scan step done");
                    if wait {
                        tokio::time::sleep(std::time::Duration::from_secs(
                            scan_blockchain_options.scan_interval,
                        ))
                        .await;
                    }
                }
                Err(e) => {
                    log::info!("Scan step failed - trying again: {}", e);
                    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
                }
            }
        }
    } else {
        if current_block < scan_info.last_block {
            log::warn!(
                "Current block {} is lower than last block from db {}, no newer data on blockchain",
                current_block,
                scan_info.last_block
            );
            return Ok(());
        }

        if scan_info.last_block > start_block {
            log::info!(
                    "Start block from db is higher than start block from cli {}, using start block from db {}",
                    start_block,
                    scan_info.last_block
                );
            start_block = scan_info.last_block;
        } else if scan_info.last_block != -1 {
            log::error!(
            "There is old entry in db, remove it to start new scan or give proper block range: start block: {}, last block {}",
            start_block,
            scan_info.last_block
        );
            return Err(err_custom_create!(
            "There is old entry in db, remove it to start new scan or give proper block range: start block: {}, last block {}",
            start_block,
            scan_info.last_block
        ));
        }

        let mut end_block = if let Some(max_block_range) = scan_blockchain_options.max_block_range {
            start_block + max_block_range as i64
        } else {
            current_block
        };

        if let Some(blocks_behind) = scan_blockchain_options.blocks_behind {
            if end_block > current_block - blocks_behind as i64 {
                log::info!(
                "End block {} is too close to current block {}, using current block - blocks_behind: {}",
                end_block,
                current_block,
                current_block - blocks_behind as i64
            );
                end_block = current_block - blocks_behind as i64;
            }
        }

        scan_int(
            conn.clone(),
            &scan_blockchain_options,
            chain_cfg.clone(),
            web3.clone(),
            start_block,
            end_block,
            sender,
        )
        .await?;
    }

    Ok(())
}
