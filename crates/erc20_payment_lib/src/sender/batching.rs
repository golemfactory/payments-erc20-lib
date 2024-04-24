use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;

use erc20_payment_lib_common::ops::*;

use crate::error::{AllowanceRequest, ErrorBag, PaymentError};

use crate::transaction::{
    create_close_deposit, create_erc20_deposit_transfer, create_erc20_transfer,
    create_erc20_transfer_multi, create_erc20_transfer_multi_deposit, create_eth_transfer,
    MultiTransferArgs, MultiTransferDepositArgs, SingleTransferDepositArgs,
};

use crate::setup::PaymentSetup;
use crate::{err_create, err_custom_create, err_from};

use sqlx::SqlitePool;
use tokio::sync::mpsc;

use crate::signer::SignerAccount;
use erc20_payment_lib_common::model::{DepositId, TokenTransferDbObj};
use erc20_payment_lib_common::DriverEvent;
use web3::types::{Address, U256};

#[derive(Eq, Hash, PartialEq, Debug, Clone)]
pub struct TokenTransferKey {
    pub from_addr: String,
    pub receiver_addr: String,
    pub chain_id: i64,
    pub token_addr: Option<String>,
    pub deposit_id: Option<String>,
}

#[derive(Eq, Hash, PartialEq, Debug, Clone)]
pub struct TokenTransferMultiKey {
    pub from_addr: String,
    pub chain_id: i64,
    pub token_addr: Option<String>,
    pub deposit_id: Option<String>,
}

type TokenTransferMap = HashMap<TokenTransferKey, Vec<TokenTransferDbObj>>;

#[derive(Debug, Clone)]
pub struct TokenTransferMultiOrder {
    receiver: Address,
    token_transfers: Vec<TokenTransferDbObj>,
}

pub async fn gather_transactions_pre(
    account: &SignerAccount,
    chain_id: i64,
    conn: &SqlitePool,
    _payment_setup: &PaymentSetup,
    process_tx_needed: &mut bool,
) -> Result<TokenTransferMap, PaymentError> {
    let mut transfer_map = TokenTransferMap::new();

    let mut token_transfers = get_pending_token_transfers(conn, account.address, chain_id)
        .await
        .map_err(err_from!())?;

    for f in token_transfers.iter_mut() {
        match Address::from_str(&f.from_addr) {
            Ok(from_addr) => {
                if from_addr == Address::zero() {
                    f.error = Some("from_addr is zero".to_string());
                    update_token_transfer(conn, f).await.map_err(err_from!())?;
                    continue;
                }
                //@TODO: check if from_addr is in a wallet
                /*
                if from_addr != payment_setup.pub_address {
                    f.error = Some("no from_addr in wallet".to_string());
                    update_token_transfer(conn, f).await.map_err(err_from!())?;
                    continue;
                }*/
            }
            Err(_err) => {
                f.error = Some("Invalid from address".to_string());
                update_token_transfer(conn, f).await.map_err(err_from!())?;
                continue;
            }
        }
        if let Some(deposit_id) = f.deposit_id.as_ref() {
            if f.deposit_finish > 0
                && Address::from_str(&f.receiver_addr).ok() == Some(Address::zero())
                && U256::from_dec_str(&f.token_amount).ok() == Some(U256::zero())
            {
                log::info!(
                    "Creating close deposit transaction for deposit id: {}",
                    deposit_id
                );

                let deposit_id_obj = DepositId::from_db_string(deposit_id)
                    .map_err(|_| err_custom_create!("Invalid deposit id: {}", deposit_id))?;

                let from_addr = Address::from_str(&f.from_addr).map_err(err_from!())?;

                let close_deposit_tx_id = create_close_deposit(
                    from_addr,
                    deposit_id_obj.lock_address,
                    f.chain_id as u64,
                    None,
                    deposit_id_obj.deposit_id,
                )?;

                let mut transaction = conn.begin().await.map_err(err_from!())?;

                let new_tx = insert_tx(&mut *transaction, &close_deposit_tx_id)
                    .await
                    .map_err(err_from!())?;

                let mut tt = f.clone();
                tt.tx_id = Some(new_tx.id);
                update_token_transfer(&mut *transaction, &tt)
                    .await
                    .map_err(err_from!())?;

                transaction.commit().await.map_err(err_from!())?;
                *process_tx_needed = true;
                continue;
            }
        }
        match Address::from_str(&f.receiver_addr) {
            Ok(rec_address) => {
                if rec_address == Address::zero() {
                    f.error = Some("receiver_addr is zero".to_string());
                    update_token_transfer(conn, f).await.map_err(err_from!())?;
                    continue;
                }
            }
            Err(_err) => {
                f.error = Some("Invalid receiver address".to_string());
                update_token_transfer(conn, f).await.map_err(err_from!())?;
                continue;
            }
        }

        //group transactions
        let key = TokenTransferKey {
            from_addr: f.from_addr.clone(),
            receiver_addr: f.receiver_addr.clone(),
            chain_id: f.chain_id,
            token_addr: f.token_addr.clone(),
            deposit_id: f.deposit_id.clone(),
        };
        match transfer_map.get_mut(&key) {
            Some(v) => {
                v.push(f.clone());
            }
            None => {
                transfer_map.insert(key, vec![f.clone()]);
            }
        }
    }
    Ok(transfer_map)
}

pub async fn gather_transactions_batch_multi(
    conn: &SqlitePool,
    payment_setup: &PaymentSetup,
    multi_order_vector: &mut [TokenTransferMultiOrder],
    token_transfer: &TokenTransferMultiKey,
) -> Result<u32, PaymentError> {
    let chain_setup = payment_setup
        .chain_setup
        .get(&token_transfer.chain_id)
        .ok_or(err_custom_create!(
            "No setup found for chain id: {}",
            token_transfer.chain_id
        ))?;

    let use_direct_method = payment_setup.contract_use_direct_method;
    let use_unpacked_method = payment_setup.contract_use_unpacked_method;

    let max_per_batch = chain_setup.multi_contract_max_at_once;
    log::debug!("Processing token transfer {:?}", token_transfer);
    if let Some(token_addr) = token_transfer.token_addr.as_ref() {
        if !payment_setup.skip_multi_contract_check {
            if token_transfer.deposit_id.is_some() {
                //no allowance needed, because we are paying from locked deposit
            } else if let Some(multi_contract_address) = chain_setup.multi_contract_address.as_ref()
            {
                //this is some arbitrary number.
                let minimum_allowance: U256 = U256::max_value() / U256::from(2);

                let db_allowance = find_allowance(
                    conn,
                    &token_transfer.from_addr,
                    token_addr,
                    &format!("{multi_contract_address:#x}"),
                    token_transfer.chain_id,
                )
                .await
                .map_err(err_from!())?;

                let mut allowance_not_met = false;
                match db_allowance {
                    Some(db_allowance) => match db_allowance.confirm_date {
                        Some(_) => {
                            let allowance =
                                U256::from_dec_str(&db_allowance.allowance).map_err(err_from!())?;
                            if allowance < minimum_allowance {
                                log::debug!(
                                    "Allowance already confirmed from db, but it is too small"
                                );
                                allowance_not_met = true;
                            } else {
                                log::debug!("Allowance confirmed from db");
                            }
                        }
                        None => {
                            log::debug!("Allowance request found, but not confirmed");
                            allowance_not_met = true;
                        }
                    },
                    None => {
                        log::debug!("Allowance not found in db");
                        allowance_not_met = true;
                    }
                };
                if allowance_not_met {
                    return Err(err_create!(AllowanceRequest {
                        owner: token_transfer.from_addr.clone(),
                        token_addr: token_addr.clone(),
                        spender_addr: format!("{multi_contract_address:#x}"),
                        chain_id: token_transfer.chain_id,
                        amount: U256::max_value(),
                    }));
                }
            }
        }

        let split_orders = multi_order_vector
            .chunks_mut(max_per_batch)
            .collect::<Vec<_>>();

        for smaller_order in split_orders {
            let mut erc20_to = Vec::with_capacity(smaller_order.len());
            let mut erc20_amounts = Vec::with_capacity(smaller_order.len());
            let mut is_deposit_finish = false;
            for token_t in &mut *smaller_order {
                let mut sum = U256::zero();
                for token_transfer in &token_t.token_transfers {
                    sum += U256::from_dec_str(&token_transfer.token_amount).map_err(err_from!())?;
                    //close deposit if needed
                    if token_transfer.deposit_finish > 0 {
                        is_deposit_finish = true;
                    }
                }
                erc20_to.push(token_t.receiver);
                erc20_amounts.push(sum);
            }

            let mut use_transfer_for_single_payment = payment_setup.use_transfer_for_single_payment;
            if !use_transfer_for_single_payment && chain_setup.multi_contract_address.is_none() {
                log::warn!(
                    "Multi contract not set overwriting use_transfer_for_single_payment to true"
                );
                use_transfer_for_single_payment = true;
            }

            let web3tx = if erc20_to.is_empty() {
                return Ok(0);
            } else if use_transfer_for_single_payment && erc20_to.len() == 1 {
                log::info!(
                    "Inserting transaction stub for ERC20 transfer to: {:?}",
                    erc20_to[0]
                );

                if let Some(deposit_id) = token_transfer.deposit_id.as_ref() {
                    let deposit_id_obj = DepositId::from_db_string(deposit_id)
                        .map_err(|err| err_custom_create!("Invalid deposit id: {}", err))?;
                    create_erc20_deposit_transfer(SingleTransferDepositArgs {
                        from: Address::from_str(&token_transfer.from_addr).map_err(err_from!())?,
                        lock_contract: deposit_id_obj.lock_address,
                        erc20_to: erc20_to[0],
                        erc20_amount: erc20_amounts[0],
                        chain_id: token_transfer.chain_id as u64,
                        gas_limit: None,
                        deposit_id: deposit_id_obj.deposit_id,
                        deposit_finish: is_deposit_finish,
                    })?
                } else {
                    create_erc20_transfer(
                        Address::from_str(&token_transfer.from_addr).map_err(err_from!())?,
                        Address::from_str(token_addr).map_err(err_from!())?,
                        erc20_to[0],
                        erc20_amounts[0],
                        token_transfer.chain_id as u64,
                        None,
                    )?
                }
            } else if let Some(deposit_id) = token_transfer.deposit_id.as_ref() {
                let deposit_id_obj = DepositId::from_db_string(deposit_id)
                    .map_err(|err| err_custom_create!("Invalid deposit id: {}", err))?;
                log::info!(
                    "Inserting transaction stub for ERC20 multi payment: {:?} for {} distinct transfers",
                    deposit_id_obj.lock_address,
                    erc20_to.len(),
                );

                create_erc20_transfer_multi_deposit(MultiTransferDepositArgs {
                    from: Address::from_str(&token_transfer.from_addr).map_err(err_from!())?,
                    lock_contract: deposit_id_obj.lock_address,
                    erc20_to,
                    erc20_amount: erc20_amounts,
                    chain_id: token_transfer.chain_id as u64,
                    gas_limit: None,
                    deposit_id: deposit_id_obj.deposit_id,
                    deposit_finish: is_deposit_finish,
                })?
            } else if let Some(multi_contract_address) = chain_setup.multi_contract_address {
                log::info!(
                    "Inserting transaction stub for ERC20 multi transfer contract: {:?} for {} distinct transfers",
                    multi_contract_address,
                    erc20_to.len()
                );

                create_erc20_transfer_multi(MultiTransferArgs {
                    from: Address::from_str(&token_transfer.from_addr).map_err(err_from!())?,
                    contract: multi_contract_address,
                    erc20_to,
                    erc20_amount: erc20_amounts,
                    chain_id: token_transfer.chain_id as u64,
                    gas_limit: None,
                    direct: use_direct_method,
                    unpacked: use_unpacked_method,
                })?
            } else {
                log::error!(
                    "Multi contract address not set, but it is needed to process transactions"
                );
                return Err(err_custom_create!(
                    "Multi contract address not set, but it is needed to process transactions"
                ));
            };
            let mut db_transaction = conn.begin().await.map_err(err_from!())?;
            let web3_tx_dao = insert_tx(&mut *db_transaction, &web3tx)
                .await
                .map_err(err_from!())?;

            for token_t in &mut *smaller_order {
                for token_transfer in &mut token_t.token_transfers {
                    token_transfer.tx_id = Some(web3_tx_dao.id);
                    update_token_transfer(&mut *db_transaction, token_transfer)
                        .await
                        .map_err(err_from!())?;
                }
            }
            db_transaction.commit().await.map_err(err_from!())?;
        }
    } else {
        return Err(err_custom_create!("Not implemented for multi"));
    };

    Ok(1)
}

pub async fn gather_transactions_batch(
    _event_sender: Option<mpsc::Sender<DriverEvent>>,
    conn: &SqlitePool,
    payment_setup: &PaymentSetup,
    token_transfers: &mut [TokenTransferDbObj],
    token_transfer: &TokenTransferKey,
) -> Result<u32, PaymentError> {
    let mut sum = U256::zero();
    for token_transfer in token_transfers.iter() {
        sum += U256::from_dec_str(&token_transfer.token_amount).map_err(err_from!())?;
    }

    let Some(_chain_setup) = payment_setup.chain_setup.get(&token_transfer.chain_id) else {
        return Err(err_custom_create!(
            "No setup found for chain id: {}",
            token_transfer.chain_id
        ));
    };

    log::debug!("Processing token transfer {:?}", token_transfer);

    let web3tx = if let Some(token_addr) = token_transfer.token_addr.as_ref() {
        if let Some(deposit_id) = token_transfer.deposit_id.as_ref() {
            let deposit_id_obj = DepositId::from_db_string(deposit_id)
                .map_err(|err| err_custom_create!("Invalid deposit id: {}", err))?;

            let is_deposit_finish = token_transfers.iter().any(|f| f.deposit_finish > 0);
            create_erc20_deposit_transfer(SingleTransferDepositArgs {
                from: Address::from_str(&token_transfer.from_addr).map_err(err_from!())?,
                lock_contract: deposit_id_obj.lock_address,
                erc20_to: Address::from_str(&token_transfer.receiver_addr).map_err(err_from!())?,
                erc20_amount: sum,
                chain_id: token_transfer.chain_id as u64,
                gas_limit: None,
                deposit_id: deposit_id_obj.deposit_id,
                deposit_finish: is_deposit_finish,
            })?
        } else {
            create_erc20_transfer(
                Address::from_str(&token_transfer.from_addr).map_err(err_from!())?,
                Address::from_str(token_addr).map_err(err_from!())?,
                Address::from_str(&token_transfer.receiver_addr).map_err(err_from!())?,
                sum,
                token_transfer.chain_id as u64,
                None,
            )?
        }
    } else {
        create_eth_transfer(
            Address::from_str(&token_transfer.from_addr).map_err(err_from!())?,
            Address::from_str(&token_transfer.receiver_addr).map_err(err_from!())?,
            token_transfer.chain_id as u64,
            None,
            sum,
        )
    };
    let mut db_transaction = conn.begin().await.map_err(err_from!())?;
    let web3_tx_dao = insert_tx(&mut *db_transaction, &web3tx)
        .await
        .map_err(err_from!())?;
    for token_transfer in token_transfers.iter_mut() {
        token_transfer.tx_id = Some(web3_tx_dao.id);
        update_token_transfer(&mut *db_transaction, token_transfer)
            .await
            .map_err(err_from!())?;
    }
    db_transaction.commit().await.map_err(err_from!())?;
    Ok(1)
}

pub async fn gather_transactions_post(
    event_sender: Option<tokio::sync::mpsc::Sender<DriverEvent>>,
    conn: &SqlitePool,
    payment_setup: &PaymentSetup,
    token_transfer_map: &mut TokenTransferMap,
) -> Result<u32, PaymentError> {
    let mut inserted_tx_count = 0;

    let mut sorted_order = BTreeMap::<i64, TokenTransferKey>::new();

    for pair in token_transfer_map.iter() {
        let token_transfers = pair.1;
        let token_transfer = pair.0;
        let min_id = token_transfers
            .iter()
            .map(|f| f.id)
            .min()
            .ok_or_else(|| err_custom_create!("Failed algorithm when searching min"))?;
        sorted_order.insert(min_id, token_transfer.clone());
    }
    let use_multi = true;
    if use_multi {
        let mut multi_key_map =
            HashMap::<TokenTransferMultiKey, Vec<TokenTransferMultiOrder>>::new();
        for key in &sorted_order {
            let multi_key = TokenTransferMultiKey {
                from_addr: key.1.from_addr.clone(),
                chain_id: key.1.chain_id,
                token_addr: key.1.token_addr.clone(),
                deposit_id: key.1.deposit_id.clone(),
            };
            if multi_key.token_addr.is_none() {
                let token_transfer = key.1;
                let token_transfers = token_transfer_map
                    .get_mut(token_transfer)
                    .ok_or_else(|| err_custom_create!("Failed algorithm when getting key"))?;

                //sum of transfers
                match gather_transactions_batch(
                    event_sender.clone(),
                    conn,
                    payment_setup,
                    token_transfers,
                    token_transfer,
                )
                .await
                {
                    Ok(_) => {
                        inserted_tx_count += 1;
                    }
                    Err(e) => {
                        match &e.inner {
                            ErrorBag::NoAllowanceFound(_allowance_request) => {
                                //pass allowance error up
                                return Err(e);
                            }
                            _ => {
                                //mark other errors in db to not process these failed transfers again
                                /*for token_transfer in token_transfers {
                                    token_transfer.error =
                                        Some("Error in gathering transactions".to_string());
                                    update_token_transfer(conn, token_transfer)
                                        .await
                                        .map_err(err_from!())?;
                                }*/
                                //send_driver_event(
                                //    &event_sender,
                                //    DriverEventContent::TransactionFailed(TransactionFailedReason::InvalidChainId(
                                //        format!("Failed to gather transactions")
                                //    )),
                                //)
                                log::error!("Failed to gather transactions: {:?}", e);
                                tokio::time::sleep(std::time::Duration::from_secs(
                                    payment_setup.process_interval_after_error,
                                ))
                                .await;
                            }
                        }
                    }
                }
                inserted_tx_count += 1;
                continue;
            }

            //todo - fix unnecessary clone here
            let opt = token_transfer_map.get(key.1).unwrap().clone();
            token_transfer_map.remove(key.1);

            match multi_key_map.get_mut(&multi_key) {
                Some(v) => {
                    v.push(TokenTransferMultiOrder {
                        receiver: Address::from_str(&key.1.receiver_addr).map_err(err_from!())?,
                        token_transfers: opt,
                    });
                }
                None => {
                    multi_key_map.insert(
                        multi_key,
                        vec![TokenTransferMultiOrder {
                            token_transfers: opt,
                            receiver: Address::from_str(&key.1.receiver_addr)
                                .map_err(err_from!())?,
                        }],
                    );
                }
            }
        }
        for key in multi_key_map {
            let token_transfer = key.0;
            let mut token_transfers = key.1.clone();
            //todo fix clones
            match gather_transactions_batch_multi(
                conn,
                payment_setup,
                &mut token_transfers,
                &token_transfer,
            )
            .await
            {
                Ok(_) => {
                    inserted_tx_count += 1;
                }
                Err(e) => {
                    match &e.inner {
                        ErrorBag::NoAllowanceFound(_allowance_request) => {
                            //pass allowance error up
                            return Err(e);
                        }
                        _ => {
                            //mark other errors in db to not process these failed transfers again
                            for multi in token_transfers {
                                for token_transfer in multi.token_transfers {
                                    let mut tt = token_transfer.clone();
                                    tt.error = Some("Error in gathering transactions".to_string());
                                    update_token_transfer(conn, &tt)
                                        .await
                                        .map_err(err_from!())?;
                                }
                            }
                            log::error!("Failed to gather transactions: {:?}", e);
                            tokio::time::sleep(std::time::Duration::from_secs(
                                payment_setup.process_interval_after_error,
                            ))
                            .await;
                        }
                    }
                }
            }
            inserted_tx_count += 1;
        }
    }

    Ok(inserted_tx_count)
}
