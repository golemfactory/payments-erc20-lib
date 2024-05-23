use crate::contracts::*;
use crate::error::*;
use crate::eth::{get_eth_addr_from_secret, GetBalanceArgs};
use crate::multi::pack_transfers_for_multi_contract;
use crate::runtime::{
    get_token_balance, get_unpaid_token_amount, remove_transaction_force, send_driver_event,
};
use crate::signer::Signer;
use crate::utils::{datetime_from_u256_timestamp, ConversionError, StringConvExt, U256ConvExt};
use crate::{err_custom_create, err_from};
use chrono::Utc;
use erc20_payment_lib_common::model::{
    ChainTransferDbObj, ChainTxDbObj, DepositId, TokenTransferDbObj, TxDbObj,
};
use erc20_payment_lib_common::CantSignContent;
use erc20_payment_lib_common::{
    DriverEvent, DriverEventContent, NoGasDetails, NoTokenDetails, TransactionStuckReason,
};
use erc20_rpc_pool::Web3RpcPool;
use rust_decimal::Decimal;
use secp256k1::SecretKey;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::mpsc;
use web3::transports::Http;
use web3::types::{
    Address, BlockId, BlockNumber, Bytes, CallRequest, TransactionId, TransactionParameters, H160,
    H256, U256, U64,
};
use web3::Web3;

fn decode_data_to_bytes(web3_tx_dao: &TxDbObj) -> Result<Option<Bytes>, PaymentError> {
    Ok(if let Some(data) = &web3_tx_dao.call_data {
        let hex_data = hex::decode(data)
            .map_err(|_err| err_custom_create!("Failed to convert data from hex"))?;
        Some(Bytes(hex_data))
    } else {
        None
    })
}

pub fn dao_to_call_request(web3_tx_dao: &TxDbObj) -> Result<CallRequest, PaymentError> {
    Ok(CallRequest {
        from: Some(Address::from_str(&web3_tx_dao.from_addr).map_err(err_from!())?),
        to: Some(Address::from_str(&web3_tx_dao.to_addr).map_err(err_from!())?),
        gas: web3_tx_dao.gas_limit.map(U256::from),
        gas_price: None,
        value: Some(U256::from_dec_str(&web3_tx_dao.val).map_err(err_from!())?),
        data: decode_data_to_bytes(web3_tx_dao)?,
        transaction_type: Some(U64::from(2)),
        access_list: None,
        max_fee_per_gas: Some(
            U256::from_dec_str(
                &web3_tx_dao
                    .max_fee_per_gas
                    .clone()
                    .ok_or(err_custom_create!("max_fee_per_gas has to be set"))?,
            )
            .map_err(err_from!())?,
        ),
        max_priority_fee_per_gas: Some(
            U256::from_dec_str(
                &web3_tx_dao
                    .priority_fee
                    .clone()
                    .ok_or(err_custom_create!("priority_fee has to be set"))?,
            )
            .map_err(err_from!())?,
        ),
    })
}

pub fn dao_to_transaction(web3_tx_dao: &TxDbObj) -> Result<TransactionParameters, PaymentError> {
    Ok(TransactionParameters {
        nonce: Some(U256::from(
            web3_tx_dao
                .nonce
                .ok_or_else(|| err_custom_create!("Missing nonce"))?,
        )),
        to: Some(Address::from_str(&web3_tx_dao.to_addr).map_err(err_from!())?),
        gas: U256::from(
            web3_tx_dao
                .gas_limit
                .ok_or(err_custom_create!("Missing gas limit"))?,
        ),
        gas_price: None,
        value: U256::from_dec_str(&web3_tx_dao.val).map_err(err_from!())?,
        data: decode_data_to_bytes(web3_tx_dao)?.unwrap_or_default(),
        chain_id: Some(web3_tx_dao.chain_id as u64),
        transaction_type: Some(U64::from(2)),
        access_list: None,
        max_fee_per_gas: Some(
            U256::from_dec_str(
                &web3_tx_dao
                    .max_fee_per_gas
                    .clone()
                    .ok_or(err_custom_create!("max_fee_per_gas has to be set"))?,
            )
            .map_err(err_from!())?,
        ),
        max_priority_fee_per_gas: Some(
            U256::from_dec_str(
                &web3_tx_dao
                    .priority_fee
                    .clone()
                    .ok_or(err_custom_create!("priority_fee has to be set"))?,
            )
            .map_err(err_from!())?,
        ),
    })
}

// token_addr NULL means standard (non ERC20) transfer of main chain currency (i.e ETH)
pub fn create_token_transfer(
    from: Address,
    receiver: Address,
    chain_id: i64,
    payment_id: Option<&str>,
    token_addr: Option<Address>,
    token_amount: U256,
    deposit_id: Option<DepositId>,
) -> TokenTransferDbObj {
    TokenTransferDbObj {
        id: 0,
        payment_id: payment_id.map(|s| s.to_string()),
        from_addr: format!("{from:#x}"),
        receiver_addr: format!("{receiver:#x}"),
        chain_id,
        token_addr: token_addr.map(|addr| format!("{addr:#x}")),
        token_amount: token_amount.to_string(),
        deposit_id: deposit_id.map(|d| d.to_db_string()),
        deposit_finish: 0,
        create_date: Utc::now(),
        tx_id: None,
        paid_date: None,
        fee_paid: None,
        error: None,
    }
}

pub fn create_eth_transfer(
    from: Address,
    to: Address,
    chain_id: u64,
    gas_limit: Option<u64>,
    amount: U256,
) -> TxDbObj {
    TxDbObj {
        method: "transfer".to_string(),
        from_addr: format!("{from:#x}"),
        to_addr: format!("{to:#x}"),
        chain_id: chain_id as i64,
        gas_limit: gas_limit.map(|gas_limit| gas_limit as i64),
        val: amount.to_string(),
        ..Default::default()
    }
}

pub fn create_erc20_transfer(
    from: Address,
    token: Address,
    erc20_to: Address,
    erc20_amount: U256,
    chain_id: u64,
    gas_limit: Option<u64>,
) -> Result<TxDbObj, PaymentError> {
    Ok(TxDbObj {
        method: "ERC20.transfer".to_string(),
        from_addr: format!("{from:#x}"),
        to_addr: format!("{token:#x}"),
        chain_id: chain_id as i64,
        gas_limit: gas_limit.map(|gas_limit| gas_limit as i64),
        call_data: Some(hex::encode(
            encode_erc20_transfer(erc20_to, erc20_amount).map_err(err_from!())?,
        )),
        ..Default::default()
    })
}

pub struct SingleTransferDepositArgs {
    pub from: Address,
    pub lock_contract: Address,
    pub erc20_to: Address,
    pub erc20_amount: U256,
    pub chain_id: u64,
    pub gas_limit: Option<u64>,
    pub deposit_id: U256,
    pub deposit_finish: bool,
}

pub fn create_erc20_deposit_transfer(
    single_args: SingleTransferDepositArgs,
) -> Result<TxDbObj, PaymentError> {
    if !single_args.deposit_finish {
        Ok(TxDbObj {
            method: "LOCK.depositSingleTransfer".to_string(),
            from_addr: format!("{:#x}", single_args.from),
            to_addr: format!("{:#x}", single_args.lock_contract),
            chain_id: single_args.chain_id as i64,
            gas_limit: single_args.gas_limit.map(|gas_limit| gas_limit as i64),
            call_data: Some(hex::encode(
                encode_payout_single(
                    single_args.deposit_id,
                    single_args.erc20_to,
                    single_args.erc20_amount,
                )
                .map_err(err_from!())?,
            )),
            ..Default::default()
        })
    } else {
        Ok(TxDbObj {
            method: "LOCK.depositSingleTransferAndClose".to_string(),
            from_addr: format!("{:#x}", single_args.from),
            to_addr: format!("{:#x}", single_args.lock_contract),
            chain_id: single_args.chain_id as i64,
            gas_limit: single_args.gas_limit.map(|gas_limit| gas_limit as i64),
            call_data: Some(hex::encode(
                encode_payout_single_and_close(
                    single_args.deposit_id,
                    single_args.erc20_to,
                    single_args.erc20_amount,
                )
                .map_err(err_from!())?,
            )),
            ..Default::default()
        })
    }
}

pub struct MultiTransferDepositArgs {
    pub from: Address,
    pub lock_contract: Address,
    pub erc20_to: Vec<Address>,
    pub erc20_amount: Vec<U256>,
    pub chain_id: u64,
    pub gas_limit: Option<u64>,
    pub deposit_id: U256,
    pub deposit_finish: bool,
}

pub fn create_erc20_transfer_multi_deposit(
    multi_args: MultiTransferDepositArgs,
) -> Result<TxDbObj, PaymentError> {
    let (packed, _sum) =
        pack_transfers_for_multi_contract(multi_args.erc20_to, multi_args.erc20_amount)?;

    if !multi_args.deposit_finish {
        let data = encode_deposit_transfer(multi_args.deposit_id, packed).map_err(err_from!())?;
        Ok(TxDbObj {
            method: "LOCK.depositTransfer".to_string(),
            from_addr: format!("{:#x}", multi_args.from),
            to_addr: format!("{:#x}", multi_args.lock_contract),
            chain_id: multi_args.chain_id as i64,
            gas_limit: multi_args.gas_limit.map(|gas_limit| gas_limit as i64),
            call_data: Some(hex::encode(data)),
            ..Default::default()
        })
    } else {
        let data = encode_deposit_transfer_and_close(multi_args.deposit_id, packed)
            .map_err(err_from!())?;
        Ok(TxDbObj {
            method: "LOCK.depositTransferAndClose".to_string(),
            from_addr: format!("{:#x}", multi_args.from),
            to_addr: format!("{:#x}", multi_args.lock_contract),
            chain_id: multi_args.chain_id as i64,
            gas_limit: multi_args.gas_limit.map(|gas_limit| gas_limit as i64),
            call_data: Some(hex::encode(data)),
            ..Default::default()
        })
    }
}

pub struct MultiTransferArgs {
    pub from: Address,
    pub contract: Address,
    pub erc20_to: Vec<Address>,
    pub erc20_amount: Vec<U256>,
    pub chain_id: u64,
    pub gas_limit: Option<u64>,
    pub direct: bool,
    pub unpacked: bool,
}

/// Defaults direct to false and unpacked to false
pub fn create_erc20_transfer_multi(multi_args: MultiTransferArgs) -> Result<TxDbObj, PaymentError> {
    let (data, method_str) = if multi_args.unpacked {
        if multi_args.direct {
            (
                encode_multi_direct(multi_args.erc20_to, multi_args.erc20_amount)
                    .map_err(err_from!())?,
                "MULTI.golemTransferDirect".to_string(),
            )
        } else {
            (
                encode_multi_indirect(multi_args.erc20_to, multi_args.erc20_amount)
                    .map_err(err_from!())?,
                "MULTI.golemTransferIndirect".to_string(),
            )
        }
    } else {
        let (packed, sum) =
            pack_transfers_for_multi_contract(multi_args.erc20_to, multi_args.erc20_amount)?;
        if multi_args.direct {
            (
                encode_multi_direct_packed(packed).map_err(err_from!())?,
                "MULTI.golemTransferDirectPacked".to_string(),
            )
        } else {
            //default most optimal path in polygon
            (
                encode_multi_indirect_packed(packed, sum).map_err(err_from!())?,
                "MULTI.golemTransferIndirectPacked".to_string(),
            )
        }
    };

    Ok(TxDbObj {
        method: method_str,
        from_addr: format!("{:#x}", multi_args.from),
        to_addr: format!("{:#x}", multi_args.contract),
        chain_id: multi_args.chain_id as i64,
        gas_limit: multi_args.gas_limit.map(|gas_limit| gas_limit as i64),
        call_data: Some(hex::encode(data)),
        ..Default::default()
    })
}

pub fn create_distribute_transaction(
    from: Address,
    faucet_address: Address,
    chain_id: u64,
    gas_limit: Option<u64>,
    recipients: &[Address],
    amounts: &[U256],
) -> Result<TxDbObj, PaymentError> {
    let sum_amounts = amounts.iter().fold(U256::zero(), |acc, x| acc + x);
    Ok(TxDbObj {
        method: "DISTRIBUTOR.distribute".to_string(),
        from_addr: format!("{from:#x}"),
        to_addr: format!("{faucet_address:#x}"),
        chain_id: chain_id as i64,
        gas_limit: gas_limit.map(|gas_limit| gas_limit as i64),
        call_data: Some(hex::encode(
            encode_distribute(recipients, amounts).map_err(err_from!())?,
        )),
        val: sum_amounts.to_string(),
        ..Default::default()
    })
}

pub fn create_faucet_mint(
    from: Address,
    faucet_address: Address,
    chain_id: u64,
    gas_limit: Option<u64>,
) -> Result<TxDbObj, PaymentError> {
    Ok(TxDbObj {
        method: "FAUCET.create".to_string(),
        from_addr: format!("{from:#x}"),
        to_addr: format!("{faucet_address:#x}"),
        chain_id: chain_id as i64,
        gas_limit: gas_limit.map(|gas_limit| gas_limit as i64),
        call_data: Some(hex::encode(encode_faucet_create().map_err(err_from!())?)),
        ..Default::default()
    })
}

pub fn create_create_deposit(
    from: Address,
    lock_address: Address,
    chain_id: u64,
    gas_limit: Option<u64>,
    deposit_args: CreateDepositArgs,
) -> Result<TxDbObj, PaymentError> {
    Ok(TxDbObj {
        method: "LOCK.createDeposit".to_string(),
        from_addr: format!("{from:#x}"),
        to_addr: format!("{lock_address:#x}"),
        chain_id: chain_id as i64,
        gas_limit: gas_limit.map(|gas_limit| gas_limit as i64),
        call_data: Some(hex::encode(
            encode_create_deposit(deposit_args).map_err(err_from!())?,
        )),
        ..Default::default()
    })
}

pub fn create_close_deposit(
    from: Address,
    lock_address: Address,
    chain_id: u64,
    gas_limit: Option<u64>,
    deposit_id: U256,
) -> Result<TxDbObj, PaymentError> {
    Ok(TxDbObj {
        method: "LOCK.closeDeposit".to_string(),
        from_addr: format!("{from:#x}"),
        to_addr: format!("{lock_address:#x}"),
        chain_id: chain_id as i64,
        gas_limit: gas_limit.map(|gas_limit| gas_limit as i64),
        call_data: Some(hex::encode(
            encode_close_deposit(deposit_id).map_err(err_from!())?,
        )),
        ..Default::default()
    })
}

pub fn create_terminate_deposit(
    from: Address,
    lock_address: Address,
    chain_id: u64,
    gas_limit: Option<u64>,
    deposit_nonce: u64,
) -> Result<TxDbObj, PaymentError> {
    Ok(TxDbObj {
        method: "LOCK.terminateDeposit".to_string(),
        from_addr: format!("{from:#x}"),
        to_addr: format!("{lock_address:#x}"),
        chain_id: chain_id as i64,
        gas_limit: gas_limit.map(|gas_limit| gas_limit as i64),
        call_data: Some(hex::encode(
            encode_terminate_deposit(deposit_nonce).map_err(err_from!())?,
        )),
        ..Default::default()
    })
}

pub fn create_erc20_approve(
    from: Address,
    token: Address,
    contract_to_approve: Address,
    chain_id: u64,
    gas_limit: Option<u64>,
) -> Result<TxDbObj, PaymentError> {
    Ok(TxDbObj {
        method: "ERC20.approve".to_string(),
        from_addr: format!("{from:#x}"),
        to_addr: format!("{token:#x}"),
        chain_id: chain_id as i64,
        gas_limit: gas_limit.map(|gas_limit| gas_limit as i64),
        call_data: Some(hex::encode(
            encode_erc20_approve(contract_to_approve, U256::max_value()).map_err(err_from!())?,
        )),
        ..Default::default()
    })
}

pub async fn get_no_token_details(
    web3: Arc<Web3RpcPool>,
    conn: &SqlitePool,
    web3_tx_dao: &TxDbObj,
    glm_token: Address,
    wrapper_contract_address: Option<Address>,
) -> Result<NoTokenDetails, PaymentError> {
    let args = GetBalanceArgs {
        address: Address::from_str(&web3_tx_dao.from_addr).map_err(err_from!())?,
        token_address: Some(glm_token),
        call_with_details: wrapper_contract_address,
        block_number: None,
        chain_id: Some(web3_tx_dao.chain_id as u64),
    };
    Ok(NoTokenDetails {
        tx: web3_tx_dao.clone(),
        sender: Address::from_str(&web3_tx_dao.from_addr).map_err(err_from!())?,
        token_balance: get_token_balance(web3, args)
            .await
            .map(|r| r.token_balance)
            .unwrap_or(Default::default())
            .unwrap_or(Default::default())
            .to_eth()
            .unwrap_or(Decimal::default()),
        token_needed: get_unpaid_token_amount(
            conn,
            web3_tx_dao.chain_id,
            glm_token,
            Address::from_str(&web3_tx_dao.from_addr).map_err(err_from!())?,
        )
        .await?
        .to_eth()
        .map_err(err_from!())?,
    })
}

pub async fn check_transaction(
    event_sender: &Option<mpsc::Sender<DriverEvent>>,
    conn: &SqlitePool,
    glm_token: Address,
    web3: Arc<Web3RpcPool>,
    web3_tx_dao: &mut TxDbObj,
    wrapper_contract_address: Option<Address>,
) -> Result<Option<U256>, PaymentError> {
    let call_request = dao_to_call_request(web3_tx_dao)?;
    log::debug!("Check transaction with gas estimation: {:?}", call_request);
    let mut loc_call_request = call_request.clone();
    loc_call_request.max_fee_per_gas = None;
    loc_call_request.max_priority_fee_per_gas = None;
    let gas_est = if web3_tx_dao.call_data.is_none() {
        U256::from(21000)
    } else {
        match web3.clone().eth_estimate_gas(loc_call_request, None).await {
            Ok(gas_est) => gas_est,
            Err(e) => {
                let event = if e.to_string().contains("gas required exceeds allowance") {
                    log::error!("Gas estimation failed - probably insufficient funds: {}", e);
                    return Err(err_custom_create!(
                        "Gas estimation failed - probably insufficient funds"
                    ));
                } else if web3_tx_dao.method == "FAUCET.create"
                    && e.to_string().contains("Cannot acquire more funds")
                {
                    log::warn!(
                        "Faucet create call failed - probably too much token already minted: {}",
                        e
                    );
                    remove_transaction_force(conn, web3_tx_dao.id).await?;
                    return Ok(None);
                } else if e.to_string().contains("transfer amount exceeds balance") {
                    log::warn!("Transfer amount exceed balance (chain_id: {}, sender: {:#x}). Getting details...", web3_tx_dao.chain_id, Address::from_str(&web3_tx_dao.from_addr).map_err(err_from!())?);
                    match get_no_token_details(
                        web3,
                        conn,
                        web3_tx_dao,
                        glm_token,
                        wrapper_contract_address,
                    )
                    .await
                    {
                        Ok(stuck_reason) => {
                            log::warn!(
                                "Got details. needed: {} balance: {}. needed - balance: {}",
                                stuck_reason.token_needed,
                                stuck_reason.token_balance,
                                stuck_reason.token_needed - stuck_reason.token_balance
                            );
                            DriverEventContent::TransactionStuck(TransactionStuckReason::NoToken(
                                stuck_reason,
                            ))
                        }
                        Err(e) => {
                            return Err(err_custom_create!(
                            "Error during getting details about amount exceeds balance error {}",
                            e
                        ));
                        }
                    }
                } else {
                    return Err(err_custom_create!(
                        "Gas estimation failed due to unknown error {}",
                        e
                    ));
                };
                send_driver_event(event_sender, event).await;
                return Ok(None);
            }
        }
    };

    let gas_limit = if gas_est.as_u64() == 21000 {
        gas_est
    } else {
        let gas_safety_margin: U256 = U256::from(20000);
        gas_est + gas_safety_margin
    };

    log::debug!("Set gas limit basing on gas estimation: {gas_limit}");
    web3_tx_dao.gas_limit = Some(gas_limit.as_u64() as i64);

    let max_fee_per_gas = U256::from_dec_str(
        &web3_tx_dao
            .max_fee_per_gas
            .clone()
            .ok_or(err_custom_create!("max_fee_per_gas has to be set here"))?,
    )
    .map_err(err_from!())?;
    let gas_needed_for_tx = U256::from_dec_str(&web3_tx_dao.val).map_err(err_from!())?;
    let maximum_gas_needed = gas_needed_for_tx + gas_limit * max_fee_per_gas;
    Ok(Some(maximum_gas_needed))
}

pub async fn sign_transaction_deprecated(
    web3: &Web3<Http>,
    web3_tx_dao: &mut TxDbObj,
    secret_key: &SecretKey,
) -> Result<(), PaymentError> {
    let public_addr = get_eth_addr_from_secret(secret_key);
    if web3_tx_dao.from_addr.to_lowercase() != format!("{public_addr:#x}") {
        return Err(err_custom_create!(
            "From addr not match with secret key {} != {:#x}",
            web3_tx_dao.from_addr.to_lowercase(),
            public_addr
        ));
    }

    let tx_object = dao_to_transaction(web3_tx_dao)?;
    log::debug!("Signing transaction: {:#?}", tx_object);
    // Sign the tx (can be done offline)
    let signed = web3
        .accounts()
        .sign_transaction(tx_object, secret_key)
        .await
        .map_err(err_from!())?;

    let slice: Vec<u8> = signed.raw_transaction.0;
    web3_tx_dao.signed_raw_data = Some(hex::encode(slice));
    web3_tx_dao.signed_date = Some(chrono::Utc::now());
    web3_tx_dao.tx_hash = Some(format!("{:#x}", signed.transaction_hash));
    log::debug!("Transaction signed successfully: {:#?}", web3_tx_dao);
    Ok(())
}

pub async fn sign_transaction_with_callback(
    event_sender: &Option<mpsc::Sender<DriverEvent>>,
    web3_tx_dao: &mut TxDbObj,
    signer_pub_address: H160,
    signer: Arc<Box<dyn Signer + Send + Sync + 'static>>,
) -> Result<(), PaymentError> {
    let tx_object = dao_to_transaction(web3_tx_dao)?;
    log::debug!("Signing transaction: {:#?}", tx_object);
    // Sign the tx (can be done offline)
    let sign_result = signer.sign(signer_pub_address, tx_object).await;

    let signed = match sign_result {
        Ok(s) => s,
        Err(e) => {
            send_driver_event(
                event_sender,
                DriverEventContent::CantSign(CantSignContent::Tx(web3_tx_dao.clone())),
            )
            .await;

            return Err(err_custom_create!(
                "Signing transaction failed due to unknown error: {e:?}"
            ));
        }
    };

    let slice: Vec<u8> = signed.raw_transaction.0;
    web3_tx_dao.signed_raw_data = Some(hex::encode(slice));
    web3_tx_dao.signed_date = Some(chrono::Utc::now());
    web3_tx_dao.tx_hash = Some(format!("{:#x}", signed.transaction_hash));
    log::debug!("Transaction signed successfully: {:#?}", web3_tx_dao);
    Ok(())
}

pub async fn send_transaction(
    conn: &SqlitePool,
    chain_id: i64,
    glm_token: Address,
    event_sender: Option<mpsc::Sender<DriverEvent>>,
    web3: Arc<Web3RpcPool>,
    web3_tx_dao: &mut TxDbObj,
    wrapper_contract_address: Option<Address>,
) -> Result<(), PaymentError> {
    if let Some(signed_raw_data) = web3_tx_dao.signed_raw_data.as_ref() {
        let bytes = Bytes(
            hex::decode(signed_raw_data)
                .map_err(|_err| ConversionError::from("cannot decode signed_raw_data".to_string()))
                .map_err(err_from!())?,
        );
        let result = web3.clone().eth_send_raw_transaction(bytes).await;
        web3_tx_dao.broadcast_date = Some(chrono::Utc::now());

        if let Err(e) = result {
            //if e.message.contains("insufficient funds") {
            //    send_driver_event(&event_sender, DriverEvent::InsufficientFunds).await;
            //
            match e {
                web3::Error::Rpc(e) => {
                    log::error!("Error sending transaction: {:#?}", e);
                    let event = if e.message.contains("insufficient funds") {
                        Some(DriverEventContent::TransactionStuck(
                            TransactionStuckReason::NoGas(NoGasDetails {
                                tx: web3_tx_dao.clone(),
                                gas_balance: web3
                                    .clone()
                                    .eth_balance(
                                        Address::from_str(&web3_tx_dao.from_addr)
                                            .map_err(err_from!())?,
                                        None,
                                    )
                                    .await
                                    .map_err(err_from!())?
                                    .to_eth()
                                    .map_err(err_from!())?,
                                gas_needed: (U256::from_dec_str(&web3_tx_dao.val)
                                    .map_err(err_from!())?
                                    + web3_tx_dao
                                        .max_fee_per_gas
                                        .clone()
                                        .ok_or(err_custom_create!("Expected max fee per gas here"))?
                                        .to_u256()
                                        .map_err(err_from!())?
                                        * U256::from(web3_tx_dao.gas_limit.ok_or(
                                            err_custom_create!("Expected gas limit here"),
                                        )?))
                                .to_eth()
                                .map_err(err_from!())?,
                            }),
                        ))
                    } else if e.message.contains("transfer amount exceeds balance") {
                        let args = GetBalanceArgs {
                            address: Default::default(),
                            token_address: Some(glm_token),
                            call_with_details: wrapper_contract_address,
                            block_number: None,
                            chain_id: Some(chain_id as u64),
                        };
                        let token_balance = get_token_balance(web3, args)
                            .await
                            .map(|r| r.token_balance)
                            .unwrap_or(Default::default())
                            .unwrap_or(Default::default());
                        Some(DriverEventContent::TransactionStuck(
                            TransactionStuckReason::NoToken(NoTokenDetails {
                                tx: web3_tx_dao.clone(),
                                sender: Address::from_str(&web3_tx_dao.from_addr)
                                    .map_err(err_from!())?,
                                token_balance: token_balance.to_eth().unwrap_or(Decimal::default()),
                                token_needed: get_unpaid_token_amount(
                                    conn,
                                    web3_tx_dao.chain_id,
                                    glm_token,
                                    Address::from_str(&web3_tx_dao.from_addr)
                                        .map_err(err_from!())?,
                                )
                                .await?
                                .to_eth()
                                .map_err(err_from!())?,
                            }),
                        ))
                    } else if e.message.contains("invalid sender") {
                        // transaction sent with wrong chain id
                        return Err(err_custom_create!(
                            r#"Invalid sender, seems like transaction is sending to wrong chain. \
Potentially irrecoverable error that need manual intervention.
Nonce may be set incorrectly. You can try to fix it by running command (but it may lead to unpredicted side effects):
erc20processor cleanup --remove-tx-unsafe
"#
                        ));
                    } else if e.message.contains("already known") {
                        //transaction is already in mempool, success!
                        return Ok(());
                    } else {
                        None
                    };

                    if let Some(event) = event {
                        send_driver_event(&event_sender, event).await;
                    }
                }
                _ => {
                    log::error!("Error sending transaction: {:#?}", e);
                }
            }
        }
    } else {
        return Err(err_custom_create!("No signed raw data"));
    }

    Ok(())
}

// it seems that this function is not needed at all for checking the transaction status
// instead use nonce and transaction receipt
#[allow(unused)]
pub async fn find_tx(web3: &Web3<Http>, web3_tx_dao: &mut TxDbObj) -> Result<bool, PaymentError> {
    if let Some(tx_hash) = web3_tx_dao.tx_hash.as_ref() {
        let tx_hash = web3::types::H256::from_str(tx_hash)
            .map_err(|err| ConversionError::from("Failed to convert tx hash".into()))
            .map_err(err_from!())?;
        let tx = web3
            .eth()
            .transaction(TransactionId::Hash(tx_hash))
            .await
            .map_err(err_from!())?;
        if let Some(tx) = tx {
            web3_tx_dao.block_number = tx.block_number.map(|x| x.as_u64() as i64);
            Ok(true)
        } else {
            Ok(false)
        }
    } else {
        Err(err_custom_create!("No tx hash"))
    }
}

pub async fn find_receipt(
    web3: Arc<Web3RpcPool>,
    web3_tx_dao: &mut TxDbObj,
) -> Result<Option<U256>, PaymentError> {
    if let Some(tx_hash) = web3_tx_dao.tx_hash.as_ref() {
        let tx_hash = web3::types::H256::from_str(tx_hash)
            .map_err(|_err| ConversionError::from("Cannot parse tx_hash".to_string()))
            .map_err(err_from!())?;
        let receipt = web3
            .clone()
            .eth_transaction_receipt(tx_hash)
            .await
            .map_err(err_from!())?;
        if let Some(receipt) = receipt {
            web3_tx_dao.block_number = receipt.block_number.map(|x| x.as_u64() as i64);
            web3_tx_dao.chain_status = receipt.status.map(|x| x.as_u64() as i64);
            web3_tx_dao.gas_used = receipt.gas_used.map(|x| x.as_u64() as i64);
            web3_tx_dao.effective_gas_price = receipt.effective_gas_price.map(|x| x.to_string());
            let block_info = web3
                .clone()
                .eth_block(BlockId::Number(BlockNumber::Number(U64::from(
                    web3_tx_dao
                        .block_number
                        .ok_or_else(|| err_custom_create!("Block number is None"))?
                        as u64,
                ))))
                .await
                .map_err(err_from!())?
                .ok_or_else(|| err_custom_create!("Block not found"))?;
            web3_tx_dao.block_gas_price = block_info.base_fee_per_gas.map(|x| x.to_string());
            web3_tx_dao.blockchain_date = Some(
                datetime_from_u256_timestamp(block_info.timestamp).ok_or_else(|| {
                    err_custom_create!("Cannot convert timestamp to NaiveDateTime")
                })?,
            );

            let gas_used = receipt
                .gas_used
                .ok_or_else(|| err_custom_create!("Gas used expected"))?;
            let effective_gas_price = receipt
                .effective_gas_price
                .ok_or_else(|| err_custom_create!("Effective gas price expected"))?;
            web3_tx_dao.fee_paid = Some((gas_used * effective_gas_price).to_string());
            Ok(Some(effective_gas_price))
        } else {
            web3_tx_dao.block_number = None;
            web3_tx_dao.chain_status = None;
            web3_tx_dao.fee_paid = None;
            Ok(None)
        }
    } else {
        Err(err_custom_create!("No tx hash"))
    }
}

#[allow(clippy::large_enum_variant)]
pub enum FindReceiptParseResult {
    Success((ChainTxDbObj, Vec<ChainTransferDbObj>)),
    Failure(String),
}

pub async fn find_receipt_extended(
    web3: Arc<Web3RpcPool>,
    tx_hash: H256,
    chain_id: i64,
    glm_address: Address,
) -> Result<FindReceiptParseResult, PaymentError> {
    let mut chain_tx_dao = ChainTxDbObj {
        id: -1,
        tx_hash: format!("{:#x}", tx_hash),
        method: "".to_string(),
        from_addr: "".to_string(),
        to_addr: "".to_string(),
        chain_id,
        gas_limit: None,
        gas_used: None,
        block_gas_price: None,
        effective_gas_price: None,
        max_fee_per_gas: None,
        priority_fee: None,
        val: "".to_string(),
        nonce: 0,
        checked_date: Default::default(),
        error: None,
        engine_message: None,
        engine_error: None,
        blockchain_date: Default::default(),
        block_number: 0,
        chain_status: 0,
        fee_paid: "".to_string(),
        balance_eth: None,
        balance_glm: None,
    };

    let receipt = web3
        .clone()
        .eth_transaction_receipt(tx_hash)
        .await
        .map_err(err_from!())?
        .ok_or(err_custom_create!("Receipt not found"))?;
    let tx = web3
        .clone()
        .eth_transaction(TransactionId::Hash(tx_hash))
        .await
        .map_err(err_from!())?
        .ok_or(err_custom_create!("Transaction not found"))?;
    chain_tx_dao.block_number = receipt
        .block_number
        .map(|x| x.as_u64() as i64)
        .ok_or(err_custom_create!("Block number is None"))?;

    let block_info = web3
        .clone()
        .eth_block(BlockId::Number(BlockNumber::Number(U64::from(
            chain_tx_dao.block_number as u64,
        ))))
        .await
        .map_err(err_from!())?
        .ok_or(err_custom_create!("Block not found"))?;

    //println!("Receipt: {:#?}", receipt);
    chain_tx_dao.checked_date = chrono::Utc::now();
    chain_tx_dao.blockchain_date = datetime_from_u256_timestamp(block_info.timestamp)
        .ok_or_else(|| err_custom_create!("Cannot convert timestamp to NaiveDateTime"))?;

    chain_tx_dao.from_addr = format!("{:#x}", receipt.from);

    let receipt_to = receipt
        .to
        .ok_or_else(|| err_custom_create!("Receipt to for tx {:#x} to is None", tx_hash))?;
    let tx_to = tx
        .to
        .ok_or_else(|| err_custom_create!("Transaction to for tx {:#x} to is None", tx_hash))?;
    if receipt_to != tx_to {
        return Err(err_custom_create!(
            "Receipt to not match with transaction to {:#x} != {:#x}",
            receipt_to,
            tx_to
        ));
    }
    let tx_from = tx
        .from
        .ok_or(err_custom_create!("Transaction from is None"))?;
    if tx_from != receipt.from {
        return Err(err_custom_create!(
            "Transaction from not match with receipt from {:#x} != {:#x}",
            tx_from,
            receipt.from
        ));
    }

    chain_tx_dao.to_addr = format!("{receipt_to:#x}");

    let status = receipt
        .status
        .ok_or(err_custom_create!("Receipt status is None"))?;
    if status.as_u64() > 1 {
        return Err(err_custom_create!("Receipt status unknown {:#x}", status));
    }
    chain_tx_dao.chain_status = status.as_u64() as i64;
    if tx.nonce > U256::from(i64::MAX) {
        return Err(err_custom_create!("Nonce too big"));
    }
    chain_tx_dao.nonce = tx.nonce.as_u64() as i64;

    if tx.gas > U256::from(i64::MAX) {
        return Err(err_custom_create!("Gas limit too big"));
    }

    chain_tx_dao.gas_limit = Some(tx.gas.as_u64() as i64);
    chain_tx_dao.val = tx.value.to_string();

    let gas_used = receipt
        .gas_used
        .ok_or_else(|| err_custom_create!("Gas used expected"))?;

    if gas_used > U256::from(i64::MAX) {
        return Err(err_custom_create!("Gas used too big"));
    }

    chain_tx_dao.gas_used = Some(gas_used.as_u64() as i64);

    let effective_gas_price = receipt
        .effective_gas_price
        .ok_or_else(|| err_custom_create!("Effective gas price expected"))?;

    chain_tx_dao.block_gas_price = Some(
        block_info
            .base_fee_per_gas
            .unwrap_or(U256::zero())
            .to_string(),
    );
    chain_tx_dao.effective_gas_price = Some(effective_gas_price.to_string());
    chain_tx_dao.max_fee_per_gas = tx.max_fee_per_gas.map(|x| x.to_string());
    chain_tx_dao.priority_fee = tx.max_priority_fee_per_gas.map(|x| x.to_string());
    chain_tx_dao.fee_paid = (gas_used * effective_gas_price).to_string();

    chain_tx_dao.method = if tx.input.0.len() >= 4 {
        // extract method
        format!("0x{}", hex::encode(&tx.input.0[0..4]))
    } else {
        "N/A".to_string()
    };

    //todo: move to lazy static
    let erc20_transfer_event_signature: H256 =
        H256::from_str("0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef")
            .unwrap();
    let mut transfers = Vec::<ChainTransferDbObj>::new();

    if tx.value != U256::zero() {
        transfers.push(ChainTransferDbObj {
            id: 0,
            from_addr: format!("{tx_from:#x}"),
            receiver_addr: format!("{tx_to:#x}"),
            chain_id,
            token_addr: None,
            token_amount: tx.value.to_string(),
            chain_tx_id: 0,
            fee_paid: None,
            blockchain_date: Some(chain_tx_dao.blockchain_date),
        });
    }

    for log in &receipt.logs {
        if log.address != glm_address {
            continue;
        }
        if log.topics.len() == 3 && log.topics[0] == erc20_transfer_event_signature {
            let from = Address::from_slice(&log.topics[1][12..]);
            let to = Address::from_slice(&log.topics[2][12..]);
            let amount = U256::from(log.data.0.as_slice());

            transfers.push(ChainTransferDbObj {
                id: 0,
                from_addr: format!("{from:#x}"),
                receiver_addr: format!("{to:#x}"),
                chain_id,
                token_addr: Some(format!("{:#x}", log.address)),
                token_amount: amount.to_string(),
                chain_tx_id: 0,
                fee_paid: None,
                blockchain_date: Some(chain_tx_dao.blockchain_date),
            });
        }
    }

    Ok(FindReceiptParseResult::Success((chain_tx_dao, transfers)))
}

pub async fn get_erc20_logs(
    web3: Arc<Web3RpcPool>,
    erc20_address: Address,
    topic_senders: Option<Vec<H256>>,
    topic_receivers: Option<Vec<H256>>,
    from_block: i64,
    to_block: i64,
) -> Result<Vec<web3::types::Log>, PaymentError> {
    if from_block < 0 || to_block < 0 {
        return Err(err_custom_create!("Block number cannot be negative"));
    }
    let filter = web3::types::FilterBuilder::default()
        .address(vec![erc20_address])
        .topics(
            Some(vec![H256::from_str(
                "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef",
            )
            .unwrap()]),
            topic_senders,
            topic_receivers,
            None,
        )
        .from_block(BlockNumber::Number(U64::from(from_block as u64)))
        .to_block(BlockNumber::Number(U64::from(to_block as u64)));
    web3.eth_logs(filter.build())
        .await
        .map_err(|e| err_custom_create!("Error while getting logs: {}", e))
}

pub struct ImportErc20TxsArgs {
    pub web3: Arc<Web3RpcPool>,
    pub erc20_address: Address,
    pub chain_id: i64,
    pub filter_by_senders: Option<Vec<Address>>,
    pub filter_by_receivers: Option<Vec<Address>>,
    pub start_block: i64,
    pub scan_end_block: i64,
    pub blocks_at_once: u64,
}

pub async fn import_erc20_txs(import_args: ImportErc20TxsArgs) -> Result<Vec<H256>, PaymentError> {
    let start_block = import_args.start_block;
    let option_address_to_option_h256 = |val: Option<Vec<Address>>| -> Option<Vec<H256>> {
        val.map(|accounts| {
            accounts
                .iter()
                .map(|f| {
                    let mut topic = [0u8; 32];
                    topic[12..32].copy_from_slice(&f.to_fixed_bytes());
                    H256::from(topic)
                })
                .collect()
        })
    };

    let topic_receivers = option_address_to_option_h256(import_args.filter_by_receivers);
    let topic_senders = option_address_to_option_h256(import_args.filter_by_senders);

    let current_block = import_args
        .web3
        .clone()
        .eth_block_number()
        .await
        .map_err(err_from!())?
        .as_u64() as i64;

    let mut txs = HashMap::<H256, u64>::new();

    let end_block = import_args.scan_end_block;
    if start_block > end_block {
        return Err(err_custom_create!("Start block is greater than end block"));
    }
    if end_block - start_block > import_args.blocks_at_once as i64 {
        return Err(err_custom_create!("Too many blocks to scan"));
    }
    if start_block > current_block {
        return Err(err_custom_create!(
            "Start block is greater than current block"
        ));
    }
    log::debug!("Scanning chain, blocks: {start_block} - {end_block}");
    let logs = get_erc20_logs(
        import_args.web3.clone(),
        import_args.erc20_address,
        topic_senders.clone(),
        topic_receivers.clone(),
        start_block,
        end_block,
    )
    .await?;
    for log in logs.into_iter() {
        txs.insert(
            log.transaction_hash
                .ok_or(err_custom_create!("Log without transaction hash"))?,
            log.block_number
                .ok_or(err_custom_create!("Log without block number"))?
                .as_u64(),
        );
        log::info!(
            "Found matching log entry in block: {}, tx: {}",
            log.block_number.unwrap(),
            log.block_number.unwrap()
        );
    }

    if txs.is_empty() {
        log::debug!("No logs found");
    } else {
        log::info!("Found {} transactions", txs.len());
    }

    //return transactions sorted by block number
    let mut vec = txs.into_iter().collect::<Vec<(H256, u64)>>();
    vec.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(vec.into_iter().map(|(tx, _)| tx).collect())
}
