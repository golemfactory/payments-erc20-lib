use crate::contracts::{
    decode_call_with_details, encode_call_with_details, encode_erc20_allowance,
    encode_erc20_balance_of, encode_get_attestation, encode_get_deposit_details, encode_get_schema,
    encode_get_validate_deposit_signature, encode_validate_contract,
};
use crate::error::*;
use crate::runtime::ValidateDepositResult;
use crate::{err_create, err_custom_create, err_from};
use chrono::{DateTime, Utc};
use erc20_payment_lib_common::utils::{
    datetime_from_u256_timestamp, datetime_from_u256_with_option, U256ConvExt,
};
use erc20_rpc_pool::Web3RpcPool;
use secp256k1::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};
use sha3::Digest;
use sha3::Keccak256;
use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;
use web3::ethabi;
use web3::ethabi::ParamType;
use web3::types::{Address, BlockId, BlockNumber, Bytes, CallRequest, H256, U256, U64};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetBalanceResult {
    pub gas_balance: Option<U256>,
    pub token_balance: Option<U256>,
    pub block_number: u64,
    pub block_datetime: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DepositDetails {
    pub deposit_id: String,
    pub deposit_nonce: u64,
    pub funder: Address,
    pub spender: Address,
    pub amount: String,
    pub amount_decimal: rust_decimal::Decimal,
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
    pub valid_to: u64,
}

impl DepositView {
    pub fn decode_from_bytes(bytes: &[u8]) -> Result<DepositView, PaymentError> {
        if bytes.len() != 6 * 32 {
            return Err(err_custom_create!(
                "Invalid response length: {}, expected {}",
                bytes.len(),
                6 * 32
            ));
        }

        let decoded = ethabi::decode(
            &[
                ethabi::ParamType::Uint(256),
                ethabi::ParamType::Uint(64),
                ethabi::ParamType::Address,
                ethabi::ParamType::Address,
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
            valid_to: decoded[5].clone().into_uint().unwrap().as_u64(),
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

#[derive(Debug, Serialize, Deserialize)]
struct SignatureParam {
    #[serde(rename = "type")]
    pub typ: String,
    pub name: String,
}

fn ethabi_decode_string_result(bytes: Bytes) -> Result<String, PaymentError> {
    let decoded = ethabi::decode(
        &[
            ethabi::ParamType::String,
        ],
        &bytes.0,
    )
        .map_err(|err|err_custom_create!(
            "Failed to decode deposit view from bytes, check if proper contract and contract method is called: {}",
            err
        ))?;

    if decoded.len() != 1 {
        return Err(err_custom_create!(
            "Invalid response length: {}, expected {}",
            decoded.len(),
            1
        ));
    }
    decoded[0]
        .clone()
        .into_string()
        .ok_or_else(|| err_custom_create!("Failed to decode string from bytes"))
}

pub async fn validate_deposit_eth(
    web3: Arc<Web3RpcPool>,
    deposit_id: U256,
    lock_contract_address: Address,
    validate_args: BTreeMap<String, String>,
    block_number: Option<u64>,
) -> Result<ValidateDepositResult, PaymentError> {
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

    let bytes = web3
        .clone()
        .eth_call(
            CallRequest {
                to: Some(lock_contract_address),
                data: Some(encode_get_validate_deposit_signature().unwrap().into()),
                ..Default::default()
            },
            None,
        )
        .await
        .map_err(err_from!())?;

    let str = ethabi_decode_string_result(bytes)?;

    let signature_params: Vec<SignatureParam> = serde_json::from_str(&str)
        .map_err(|err| err_custom_create!("Failed to parse signature params: {}", err))?;

    let mut matched_params: Vec<String> = Vec::new();
    let mut function_params: Vec<ethabi::Param> = Vec::new();
    let mut function_values: Vec<ethabi::Token> = Vec::new();
    for signature_param in &signature_params {
        if signature_param.name == "id" {
            let new_param = ethabi::Param {
                name: "id".to_string(),
                kind: ParamType::Uint(256),
                internal_type: None,
            };
            matched_params.push("id".to_string());
            function_params.push(new_param);
            function_values.push(ethabi::Token::Uint(deposit_id));
        } else {
            let param_name = signature_param.name.clone();
            if let Some(param_value) = validate_args.get(&param_name) {
                matched_params.push(param_name.to_string());

                if signature_param.typ == "uint128" {
                    let res_value = U256::from_dec_str(param_value);
                    let value = match res_value {
                        Ok(value) => value,
                        Err(_) => U256::from_str(param_value).map_err(|err| {
                            err_custom_create!(
                                "Invalid value for parameter {}: {}",
                                param_name,
                                err
                            )
                        })?,
                    };

                    let new_param = ethabi::Param {
                        name: param_name,
                        kind: ParamType::Uint(128),
                        internal_type: None,
                    };
                    let new_token = ethabi::Token::Uint(value);
                    function_params.push(new_param);
                    function_values.push(new_token);
                } else {
                    return Err(err_custom_create!(
                        "Unsupported type for parameter {}: {}",
                        param_name,
                        signature_param.typ
                    ));
                }
            } else {
                return Err(err_custom_create!(
                    "Missing required parameter: {}",
                    signature_param.name
                ));
            }
        }
    }

    for signature_param in &signature_params {
        if !matched_params.contains(&signature_param.name) {
            return Err(err_custom_create!(
                "Missing required parameter: {}",
                signature_param.name
            ));
        }
    }

    log::warn!("Matched params: {:?}", matched_params);
    log::warn!("Function params: {:?}", function_params);
    log::warn!("Function values: {:?}", function_values);

    log::warn!("Signature params: {:?}", signature_params);

    let res = web3
        .eth_call(
            CallRequest {
                to: Some(lock_contract_address),
                data: Some(
                    encode_validate_contract(function_params, function_values)
                        .unwrap()
                        .into(),
                ),
                ..Default::default()
            },
            Some(BlockId::Number(BlockNumber::Number(U64::from(
                block_number,
            )))),
        )
        .await
        .map_err(err_from!())?;

    let str = ethabi_decode_string_result(res)?;
    Ok(if str == "valid" {
        ValidateDepositResult::Valid
    } else {
        ValidateDepositResult::Invalid(str)
    })
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
        current_block: block_number,
        amount_decimal: amount_u256.to_eth().map_err(err_from!())?,
        current_block_datetime: None,
        valid_to,
    })
}

#[derive(Debug, Clone, Default)]
pub struct GetBalanceArgs {
    /// Address to get balance for
    pub address: Address,
    /// erc20 token address
    pub token_address: Option<Address>,
    /// optional address of the WrapperCall contract
    pub call_with_details: Option<Address>,
    /// optional block number to do the check for
    pub block_number: Option<u64>,
    /// chain id for response verification
    pub chain_id: Option<u64>,
}

async fn get_balance_using_contract_wrapper(
    web3: Arc<Web3RpcPool>,
    args: GetBalanceArgs,
    token_address: Address,
    call_with_details: Address,
) -> Result<Option<GetBalanceResult>, PaymentError> {
    let abi_encoded_get_balance = encode_erc20_balance_of(args.address).map_err(err_from!())?;

    let call_data =
        encode_call_with_details(token_address, abi_encoded_get_balance).map_err(err_from!())?;

    let block_id = if let Some(block_number) = args.block_number {
        log::debug!(
            "Checking balance (contract) for block number {}",
            block_number
        );
        Some(BlockId::Number(BlockNumber::Number(block_number.into())))
    } else {
        log::debug!("Checking balance (contract) for latest block");
        None
    };
    match web3
        .clone()
        .eth_call(
            CallRequest {
                from: Some(args.address),
                to: Some(call_with_details),
                data: Some(Bytes::from(call_data)),
                ..Default::default()
            },
            block_id,
        )
        .await
    {
        Ok(res) => {
            let (block_info, call_result) = decode_call_with_details(&res.0)?;

            if let Some(chain_id) = args.chain_id {
                if block_info.chain_id != chain_id {
                    return Err(err_custom_create!(
                        "Invalid chain id in response: {}, expected {}",
                        block_info.chain_id,
                        chain_id
                    ));
                }
            }

            let token_balance = U256::from_big_endian(&call_result);

            log::debug!(
                "Token balance response: {:?} - token balance: {}",
                block_info,
                token_balance
            );
            Ok(Some(GetBalanceResult {
                gas_balance: Some(block_info.eth_balance),
                token_balance: Some(token_balance),
                block_number: block_info.block_number,
                block_datetime: block_info.block_datetime,
            }))
        }
        Err(e) => {
            if e.to_string().to_lowercase().contains("insufficient funds") {
                log::warn!(
                    "Balance check via wrapper contract failed, falling back to standard method"
                );
                Ok(None)
            } else {
                log::error!(
                    "Error getting balance for account: {:#x} - {}",
                    args.address,
                    e
                );
                Err(err_custom_create!(
                    "Error getting balance for account: {:#x} - {}",
                    args.address,
                    e
                ))
            }
        }
    }
}

async fn get_balance_simple(
    web3: Arc<Web3RpcPool>,
    args: GetBalanceArgs,
) -> Result<GetBalanceResult, PaymentError> {
    let block_id = if let Some(block_number) = args.block_number {
        log::debug!("Checking balance for block number {}", block_number);
        BlockId::Number(BlockNumber::Number(block_number.into()))
    } else {
        log::debug!("Checking balance for latest block");
        BlockId::Number(BlockNumber::Latest)
    };
    let block_info = web3
        .clone()
        .eth_block(block_id)
        .await
        .map_err(err_from!())?
        .ok_or(err_custom_create!("Cannot found block_info"))?;

    let block_number = block_info
        .number
        .ok_or(err_custom_create!(
            "Failed to found block number in block info",
        ))?
        .as_u64();
    let gas_balance = Some(
        web3.clone()
            .eth_balance(args.address, Some(BlockNumber::Number(block_number.into())))
            .await
            .map_err(err_from!())?,
    );

    let block_number = block_info
        .number
        .ok_or(err_custom_create!(
            "Failed to found block number in block info",
        ))?
        .as_u64();

    let block_date = datetime_from_u256_timestamp(block_info.timestamp).ok_or(
        err_custom_create!("Failed to found block date in block info"),
    )?;

    let token_balance = if let Some(token_address) = args.token_address {
        let call_data = encode_erc20_balance_of(args.address).map_err(err_from!())?;
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
        block_datetime: block_date,
    })
}

pub async fn get_balance(
    web3: Arc<Web3RpcPool>,
    args: GetBalanceArgs,
) -> Result<GetBalanceResult, PaymentError> {
    log::debug!(
        "Checking balance for address {:#x}, token address: {:#x}",
        args.address,
        args.token_address.unwrap_or_default(),
    );

    let balance = if let (Some(token_address), Some(call_with_details)) =
        (args.token_address, args.call_with_details)
    {
        get_balance_using_contract_wrapper(
            web3.clone(),
            args.clone(),
            token_address,
            call_with_details,
        )
        .await?
    } else {
        None
    };

    if let Some(balance) = balance {
        Ok(balance)
    } else {
        get_balance_simple(web3, args).await
    }
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
