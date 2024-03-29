use crate::model::{AllowanceDbObj, TokenTransferDbObj, TxDbObj};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use std::collections::BTreeMap;
use web3::types::Address;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StatusProperty {
    CantSign {
        chain_id: i64,
        address: String,
    },
    NoGas {
        chain_id: i64,
        address: String,
        missing_gas: Decimal,
    },
    NoToken {
        chain_id: i64,
        address: String,
        missing_token: Decimal,
    },
    Web3RpcError {
        chain_id: i64,
        error: String,
    },
    TxStuck {
        chain_id: i64,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct SharedInfoTx {
    pub message: String,
    pub error: Option<String>,
    pub skip: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FaucetData {
    pub faucet_events: BTreeMap<String, DateTime<Utc>>,
    pub last_cleanup: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GasLowInfo {
    pub tx: TxDbObj,
    pub tx_max_fee_per_gas_gwei: Decimal,
    pub block_date: chrono::DateTime<Utc>,
    pub block_number: u64,
    pub block_base_fee_per_gas_gwei: Decimal,
    pub assumed_min_priority_fee_gwei: Decimal,
    pub user_friendly_message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoGasDetails {
    pub tx: TxDbObj,
    pub gas_balance: Decimal,
    pub gas_needed: Decimal,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoTokenDetails {
    pub tx: TxDbObj,
    pub sender: Address,
    pub token_balance: Decimal,
    pub token_needed: Decimal,
}

#[derive(Debug, Clone, Serialize)]
#[allow(clippy::large_enum_variant)]
#[serde(rename_all = "camelCase")]
pub enum TransactionStuckReason {
    NoGas(NoGasDetails),
    NoToken(NoTokenDetails),
    GasPriceLow(GasLowInfo),
    RPCEndpointProblems(String),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TransactionFailedReason {
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionFinishedInfo {
    pub token_transfer_dao: TokenTransferDbObj,
    pub tx_dao: TxDbObj,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Web3RpcPoolContent {
    Success,
    Error(String),
    AllEndpointsFailed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Web3RpcPoolInfo {
    pub chain_id: u64,
    pub content: Web3RpcPoolContent,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CantSignContent {
    Tx(TxDbObj),
    Allowance(AllowanceDbObj),
}

impl CantSignContent {
    pub fn chain_id(&self) -> i64 {
        match self {
            CantSignContent::Tx(tx) => tx.chain_id,
            CantSignContent::Allowance(allowance) => allowance.chain_id,
        }
    }

    pub fn address(&self) -> &str {
        match self {
            CantSignContent::Tx(tx) => &tx.from_addr,
            CantSignContent::Allowance(allowance) => &allowance.owner,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[allow(clippy::large_enum_variant)]
#[serde(rename_all = "camelCase")]
pub enum DriverEventContent {
    Alive,
    TransactionConfirmed(TxDbObj),
    TransferFinished(TransactionFinishedInfo),
    ApproveFinished(AllowanceDbObj),
    TransactionStuck(TransactionStuckReason),
    TransactionFailed(TransactionFailedReason),
    CantSign(CantSignContent),
    StatusChanged(Vec<StatusProperty>),
    Web3RpcMessage(Web3RpcPoolInfo),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverEvent {
    pub create_date: DateTime<Utc>,
    pub content: DriverEventContent,
}

impl DriverEvent {
    pub fn now(content: DriverEventContent) -> Self {
        DriverEvent {
            create_date: Utc::now(),
            content,
        }
    }
}
