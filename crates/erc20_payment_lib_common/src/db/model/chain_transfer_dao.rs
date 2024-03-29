use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Serialize, sqlx::FromRow, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChainTransferDbObj {
    pub id: i64,
    pub from_addr: String,
    pub receiver_addr: String,
    pub chain_id: i64,
    pub token_addr: Option<String>,
    pub token_amount: String,
    pub chain_tx_id: i64,
    pub fee_paid: Option<String>,
    pub blockchain_date: Option<DateTime<Utc>>,
}

#[derive(Serialize, sqlx::FromRow, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChainTransferDbObjExt {
    pub id: i64,
    pub from_addr: String,
    pub receiver_addr: String,
    pub chain_id: i64,
    pub token_addr: Option<String>,
    pub token_amount: String,
    pub chain_tx_id: i64,
    pub fee_paid: Option<String>,
    pub blockchain_date: Option<DateTime<Utc>>,
    pub tx_hash: String,
    pub block_number: i64,
    pub to_addr: String,
    pub caller_addr: String,
}
