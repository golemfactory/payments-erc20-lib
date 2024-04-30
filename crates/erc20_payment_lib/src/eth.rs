use crate::contracts::{
    encode_erc20_allowance, encode_erc20_balance_of, encode_get_attestation,
    encode_get_deposit_details, encode_get_schema,
};
use crate::error::*;
use crate::{err_create, err_custom_create, err_from};
use chrono::{DateTime, Utc};
use erc20_payment_lib_common::utils::{
    datetime_from_u256_timestamp, datetime_from_u256_with_option, U256ConvExt,
};
use erc20_rpc_pool::Web3RpcPool;
use secp256k1::{PublicKey, SecretKey};
use serde::Serialize;
use sha3::Digest;
use sha3::Keccak256;
use std::sync::Arc;
use web3::ethabi;
use web3::types::{Address, BlockId, BlockNumber, Bytes, CallRequest, H256, U256, U64};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetBalanceResult {
    pub gas_balance: Option<U256>,
    pub token_balance: Option<U256>,
    pub block_number: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DepositDetails {
    pub deposit_id: String,
    pub deposit_nonce: u64,
    pub funder: Address,
    pub spender: Address,
    pub amount: String,
    pub fee_amount: String,
    pub amount_decimal: rust_decimal::Decimal,
    pub fee_amount_decimal: rust_decimal::Decimal,
    pub valid_to: chrono::DateTime<chrono::Utc>,
    pub current_block: u64,
    pub current_block_datetime: Option<chrono::DateTime<chrono::Utc>>,
}

pub struct DepositView {
    pub id: U256,
    pub nonce: u64,
    pub funder: Address,
    pub spender: Address,
    pub amount: u128,
    pub fee_amount: u128,
    pub valid_to: u64,
}

impl DepositView {
    pub fn decode_from_bytes(bytes: &[u8]) -> Result<DepositView, PaymentError> {
        if bytes.len() != 7 * 32 {
            return Err(err_custom_create!(
                "Invalid response length: {}, expected {}",
                bytes.len(),
                7 * 32
            ));
        }

        let decoded = ethabi::decode(
            &[
                ethabi::ParamType::Uint(256),
                ethabi::ParamType::Uint(64),
                ethabi::ParamType::Address,
                ethabi::ParamType::Address,
                ethabi::ParamType::Uint(128),
                ethabi::ParamType::Uint(128),
                ethabi::ParamType::Uint(64),
            ],
            bytes,
        )
        .map_err(|err|err_custom_create!(
            "Failed to decode deposit view from bytes, check if proper contract and contract method is called: {}",
            err
        ))?;

        //these unwraps are safe because we know the types from the decode call
        //be careful when changing types!
        Ok(DepositView {
            id: decoded[0].clone().into_uint().unwrap(),
            nonce: decoded[1].clone().into_uint().unwrap().as_u64(),
            funder: decoded[2].clone().into_address().unwrap(),
            spender: decoded[3].clone().into_address().unwrap(),
            amount: decoded[4].clone().into_uint().unwrap().as_u128(),
            fee_amount: decoded[5].clone().into_uint().unwrap().as_u128(),
            valid_to: decoded[6].clone().into_uint().unwrap().as_u64(),
        })
    }
}

pub fn deposit_id_from_nonce(funder: Address, nonce: u64) -> U256 {
    let mut slice: [u8; 32] = [0; 32];
    slice[0..20].copy_from_slice(funder.0.as_slice());
    slice[24..32].copy_from_slice(&nonce.to_be_bytes());
    U256::from_big_endian(&slice)
}

pub fn nonce_from_deposit_id(deposit_id: U256) -> u64 {
    let mut slice: [u8; 32] = [0; 32];
    deposit_id.to_big_endian(&mut slice);
    u64::from_be_bytes(slice[24..32].try_into().unwrap())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttestationSchema {
    pub uid: H256,
    pub resolver: Address,
    pub revocable: bool,
    pub schema: String,
}

pub async fn get_schema_details(
    web3: Arc<Web3RpcPool>,
    uid: H256,
    eas_schema_contract_address: Address,
) -> Result<crate::eth::AttestationSchema, PaymentError> {
    let res = web3
        .eth_call(
            CallRequest {
                to: Some(eas_schema_contract_address),
                data: Some(encode_get_schema(uid).unwrap().into()),
                ..Default::default()
            },
            None,
        )
        .await
        .map_err(err_from!())?;

    let decoded = ethabi::decode(
        &[
            ethabi::ParamType::Tuple(
                vec![
                    ethabi::ParamType::FixedBytes(32),
                    ethabi::ParamType::Address,
                    ethabi::ParamType::Bool,
                    ethabi::ParamType::String
                ]
            )
        ],
        &res.0
    ).map_err(|err|err_custom_create!(
        "Failed to decode attestation view from bytes, check if proper contract and contract method is called: {}",
        err
    ))?;

    let decoded = decoded[0].clone().into_tuple().unwrap();
    log::info!("Decoded attestation schema: {:?}", decoded);
    let schema = AttestationSchema {
        uid: H256::from_slice(decoded[0].clone().into_fixed_bytes().unwrap().as_slice()),
        resolver: decoded[1].clone().into_address().unwrap(),
        revocable: decoded[2].clone().into_bool().unwrap(),
        schema: decoded[3].clone().into_string().unwrap(),
    };

    Ok(schema)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Attestation {
    pub uid: H256,
    pub schema: H256,
    pub time: DateTime<Utc>,
    pub expiration_time: Option<DateTime<Utc>>,
    pub revocation_time: Option<DateTime<Utc>>,
    pub ref_uid: H256,
    pub recipient: Address,
    pub attester: Address,
    pub revocable: bool,
    pub data: Bytes,
}

pub async fn get_attestation_details(
    web3: Arc<Web3RpcPool>,
    uid: H256,
    eas_contract_address: Address,
) -> Result<Option<Attestation>, PaymentError> {
    let res = web3
        .eth_call(
            CallRequest {
                to: Some(eas_contract_address),
                data: Some(encode_get_attestation(uid).unwrap().into()),
                ..Default::default()
            },
            None,
        )
        .await
        .map_err(err_from!())?;

    let decoded = ethabi::decode(
        &[
            ethabi::ParamType::Tuple(
                vec![
                    ethabi::ParamType::FixedBytes(32),
                    ethabi::ParamType::FixedBytes(32),
                    ethabi::ParamType::Uint(64),
                    ethabi::ParamType::Uint(64),
                    ethabi::ParamType::Uint(64),
                    ethabi::ParamType::FixedBytes(32),
                    ethabi::ParamType::Address,
                    ethabi::ParamType::Address,
                    ethabi::ParamType::Bool,
                    ethabi::ParamType::Bytes
                ]
            )
        ],
        &res.0
    ).map_err(|err|err_custom_create!(
        "Failed to decode attestation view from bytes, check if proper contract and contract method is called: {}",
        err
    ))?;

    let decoded = decoded[0].clone().into_tuple().unwrap();
    if decoded[0] == ethabi::Token::FixedBytes(vec![0; 32]) {
        return Ok(None);
    }
    log::info!("Decoded attestation: {:?}", decoded);
    let attestation = Attestation {
        uid: H256::from_slice(decoded[0].clone().into_fixed_bytes().unwrap().as_slice()),
        schema: H256::from_slice(decoded[1].clone().into_fixed_bytes().unwrap().as_slice()),
        time: datetime_from_u256_with_option(decoded[2].clone().into_uint().unwrap())
            .ok_or(err_custom_create!("Attestation timestamp out of range"))?,
        expiration_time: datetime_from_u256_with_option(decoded[3].clone().into_uint().unwrap()),
        revocation_time: datetime_from_u256_with_option(decoded[4].clone().into_uint().unwrap()),
        ref_uid: H256::from_slice(decoded[5].clone().into_fixed_bytes().unwrap().as_slice()),
        recipient: decoded[6].clone().into_address().unwrap(),
        attester: decoded[7].clone().into_address().unwrap(),
        revocable: decoded[8].clone().into_bool().unwrap(),
        data: Bytes::from(decoded[9].clone().into_bytes().unwrap()),
    };

    Ok(Some(attestation))
}

pub async fn get_deposit_details(
    web3: Arc<Web3RpcPool>,
    deposit_id: U256,
    lock_contract_address: Address,
    block_number: Option<u64>,
) -> Result<DepositDetails, PaymentError> {
    let block_number = if let Some(block_number) = block_number {
        log::debug!("Checking balance for block number {}", block_number);
        block_number
    } else {
        web3.clone()
            .eth_block_number()
            .await
            .map_err(err_from!())?
            .as_u64()
    };

    let res = web3
        .eth_call(
            CallRequest {
                to: Some(lock_contract_address),
                data: Some(encode_get_deposit_details(deposit_id).unwrap().into()),
                ..Default::default()
            },
            Some(BlockId::Number(BlockNumber::Number(U64::from(
                block_number,
            )))),
        )
        .await
        .map_err(err_from!())?;

    let deposit_view = DepositView::decode_from_bytes(&res.0)?;

    let amount_u256 = U256::from(deposit_view.amount);
    let fee_amount_u256 = U256::from(deposit_view.fee_amount);

    let valid_to = chrono::DateTime::from_timestamp(
        deposit_view
            .valid_to
            .try_into()
            .map_err(|e| err_custom_create!("Cast error: {e}"))?,
        0,
    )
    .ok_or_else(|| err_custom_create!("Deposit timestamp out of range"))?;

    Ok(DepositDetails {
        deposit_id: format!("{:#x}", deposit_view.id),
        deposit_nonce: deposit_view.nonce,
        funder: deposit_view.funder,
        spender: deposit_view.spender,
        amount: amount_u256.to_string(),
        fee_amount: fee_amount_u256.to_string(),
        current_block: block_number,
        amount_decimal: amount_u256.to_eth().map_err(err_from!())?,
        fee_amount_decimal: fee_amount_u256.to_eth().map_err(err_from!())?,
        current_block_datetime: None,
        valid_to,
    })
}

pub async fn get_balance(
    web3: Arc<Web3RpcPool>,
    token_address: Option<Address>,
    address: Address,
    check_gas: bool,
    block_number: Option<u64>,
) -> Result<GetBalanceResult, PaymentError> {
    log::debug!(
        "Checking balance for address {:#x}, token address: {:#x}, check_gas {}",
        address,
        token_address.unwrap_or_default(),
        check_gas,
    );
    let block_number = if let Some(block_number) = block_number {
        log::debug!("Checking balance for block number {}", block_number);
        block_number
    } else {
        web3.clone()
            .eth_block_number()
            .await
            .map_err(err_from!())?
            .as_u64()
    };

    let gas_balance = if check_gas {
        Some(
            web3.clone()
                .eth_balance(address, Some(BlockNumber::Number(block_number.into())))
                .await
                .map_err(err_from!())?,
        )
    } else {
        None
    };

    let token_balance = if let Some(token_address) = token_address {
        let call_data = encode_erc20_balance_of(address).map_err(err_from!())?;
        let res = web3
            .clone()
            .eth_call(
                CallRequest {
                    from: None,
                    to: Some(token_address),
                    gas: None,
                    gas_price: None,
                    value: None,
                    data: Some(Bytes::from(call_data)),
                    transaction_type: None,
                    access_list: None,
                    max_fee_per_gas: None,
                    max_priority_fee_per_gas: None,
                },
                Some(BlockId::Number(BlockNumber::Number(block_number.into()))),
            )
            .await
            .map_err(err_from!())?;
        if res.0.len() != 32 {
            return Err(err_create!(TransactionFailedError::new(&format!(
                "Invalid balance response: {:?}. Probably not a valid ERC20 contract {:#x}",
                res.0, token_address
            ))));
        };
        Some(U256::from_big_endian(&res.0))
    } else {
        None
    };
    Ok(GetBalanceResult {
        gas_balance,
        token_balance,
        block_number,
    })
}

pub struct Web3BlockInfo {
    pub block_number: u64,
    pub block_date: chrono::DateTime<chrono::Utc>,
}

pub async fn get_latest_block_info(web3: Arc<Web3RpcPool>) -> Result<Web3BlockInfo, PaymentError> {
    let block_info = web3
        .eth_block(BlockId::Number(BlockNumber::Latest))
        .await
        .map_err(err_from!())?
        .ok_or(err_custom_create!("Cannot found block_info"))?;

    let block_number = block_info
        .number
        .ok_or(err_custom_create!(
            "Failed to found block number in block info",
        ))?
        .as_u64();

    let block_date = datetime_from_u256_timestamp(block_info.timestamp).ok_or(
        err_custom_create!("Failed to found block date in block info"),
    )?;

    Ok(Web3BlockInfo {
        block_number,
        block_date,
    })
}

pub(crate) async fn get_transaction_count(
    address: Address,
    web3: Arc<Web3RpcPool>,
    pending: bool,
) -> Result<u64, web3::Error> {
    let nonce_type = match pending {
        true => web3::types::BlockNumber::Pending,
        false => web3::types::BlockNumber::Latest,
    };
    let nonce = web3
        .eth_transaction_count(address, Some(nonce_type))
        .await?;
    Ok(nonce.as_u64())
}

pub(crate) fn get_eth_addr_from_secret(secret_key: &SecretKey) -> Address {
    Address::from_slice(
        &Keccak256::digest(
            &PublicKey::from_secret_key(&secp256k1::Secp256k1::new(), secret_key)
                .serialize_uncompressed()[1..65],
        )
        .as_slice()[12..],
    )
}

pub async fn check_allowance(
    web3: Arc<Web3RpcPool>,
    owner: Address,
    token: Address,
    spender: Address,
) -> Result<U256, PaymentError> {
    log::debug!("Checking multi payment contract for allowance...");
    let call_request = CallRequest {
        from: Some(owner),
        to: Some(token),
        gas: None,
        gas_price: None,
        value: None,
        data: Some(Bytes(
            encode_erc20_allowance(owner, spender).map_err(err_from!())?,
        )),
        transaction_type: None,
        access_list: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
    };
    let res = web3
        .eth_call(call_request, None)
        .await
        .map_err(err_from!())?;
    if res.0.len() != 32 {
        return Err(err_custom_create!(
            "Invalid response from ERC20 allowance check {:?}",
            res
        ));
    };
    let allowance = U256::from_big_endian(&res.0);
    log::debug!(
        "Check allowance: owner: {:?}, token: {:?}, contract: {:?}, allowance: {:?}",
        owner,
        token,
        spender,
        allowance
    );

    Ok(allowance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_get_eth_addr_from_secret() {
        let sk =
            SecretKey::from_str("0000000000000000000000000000000000000000000000000000000000000001")
                .unwrap();
        let addr = format!("{:#x}", get_eth_addr_from_secret(&sk));
        assert_eq!(addr, "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf");
    }
}
