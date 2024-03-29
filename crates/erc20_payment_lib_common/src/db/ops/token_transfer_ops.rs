use super::model::TokenTransferDbObj;
use crate::db::ops::get_chain_transfers_by_chain_id;
use crate::error::PaymentError;
use crate::error::*;
use crate::{err_custom_create, err_from};
use chrono::{DateTime, Duration, Utc};
use sqlx::Executor;
use sqlx::Sqlite;
use sqlx::SqlitePool;
use std::collections::{BTreeMap, HashSet};
use std::ops::AddAssign;
use std::str::FromStr;
use web3::types::{Address, U256};

pub async fn check_if_deposit_closed<'c, E>(
    executor: E,
    chain_id: i64,
    deposit_id: &str,
) -> Result<bool, sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    let finished_count = sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(*) FROM token_transfer WHERE chain_id=$1 AND deposit_id=$2 AND deposit_finish=1",
    )
    .bind(chain_id)
    .bind(deposit_id)
    .fetch_one(executor)
    .await?;
    Ok(finished_count.0 > 0)
}

pub async fn insert_token_transfer<'c, E>(
    executor: E,
    token_transfer: &TokenTransferDbObj,
) -> Result<TokenTransferDbObj, sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    sqlx::query_as::<_, TokenTransferDbObj>(
        r"INSERT INTO token_transfer
(payment_id, from_addr, receiver_addr, chain_id, token_addr, token_amount, deposit_id, deposit_finish, create_date, tx_id, paid_date, fee_paid, error)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, strftime('%Y-%m-%dT%H:%M:%f', 'now'), $9, $10, $11, $12) RETURNING *;
",
    )
    .bind(&token_transfer.payment_id)
    .bind(&token_transfer.from_addr)
    .bind(&token_transfer.receiver_addr)
    .bind(token_transfer.chain_id)
    .bind(&token_transfer.token_addr)
    .bind(&token_transfer.token_amount)
    .bind(&token_transfer.deposit_id)
    .bind(token_transfer.deposit_finish)
    .bind(token_transfer.tx_id)
    .bind(token_transfer.paid_date)
    .bind(&token_transfer.fee_paid)
    .bind(&token_transfer.error)
    .fetch_one(executor)
    .await
}

pub async fn insert_token_transfer_with_deposit_check(
    conn: &SqlitePool,
    token_transfer: &TokenTransferDbObj,
) -> Result<TokenTransferDbObj, PaymentError> {
    if let Some(deposit_id) = token_transfer.deposit_id.as_ref() {
        let mut transaction = conn.begin().await.map_err(err_from!())?;
        let is_finished =
            check_if_deposit_closed(&mut *transaction, token_transfer.chain_id, deposit_id)
                .await
                .map_err(err_from!())?;
        if is_finished {
            return Err(err_custom_create!(
                "Cannot add token_transfer to already finished deposit"
            ));
        }
        let res = sqlx::query_as::<_, TokenTransferDbObj>(
            r"INSERT INTO token_transfer
(payment_id, from_addr, receiver_addr, chain_id, token_addr, token_amount, deposit_id, deposit_finish, create_date, tx_id, paid_date, fee_paid, error)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, strftime('%Y-%m-%dT%H:%M:%f', 'now'), $9, $10, $11, $12) RETURNING *;
",
        )
            .bind(&token_transfer.payment_id)
            .bind(&token_transfer.from_addr)
            .bind(&token_transfer.receiver_addr)
            .bind(token_transfer.chain_id)
            .bind(&token_transfer.token_addr)
            .bind(&token_transfer.token_amount)
            .bind(&token_transfer.deposit_id)
            .bind(token_transfer.deposit_finish)
            .bind(token_transfer.tx_id)
            .bind(token_transfer.paid_date)
            .bind(&token_transfer.fee_paid)
            .bind(&token_transfer.error)
            .fetch_one(&mut *transaction)
            .await.map_err(err_from!())?;
        transaction.commit().await.map_err(err_from!())?;
        Ok(res)
    } else {
        insert_token_transfer(conn, token_transfer)
            .await
            .map_err(err_from!())
    }
}

pub async fn remap_token_transfer_tx<'c, E>(
    executor: E,
    old_tx_id: i64,
    new_tx_id: i64,
) -> Result<(), sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    let _res = sqlx::query(
        r"UPDATE token_transfer SET
            tx_id = $2
            WHERE tx_id = $1
        ",
    )
    .bind(old_tx_id)
    .bind(new_tx_id)
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn cleanup_token_transfer_tx<'c, E>(executor: E, tx_id: i64) -> Result<(), sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    let _res = sqlx::query(
        r"UPDATE token_transfer SET
            tx_id = NULL,
            fee_paid = NULL,
            error = NULL,
            paid_date = NULL
            WHERE tx_id = $1
        ",
    )
    .bind(tx_id)
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn update_token_transfer<'c, E>(
    executor: E,
    token_transfer: &TokenTransferDbObj,
) -> Result<TokenTransferDbObj, sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    let _res = sqlx::query(
        r"UPDATE token_transfer SET
payment_id = $2,
from_addr = $3,
receiver_addr = $4,
chain_id = $5,
token_addr = $6,
token_amount = $7,
deposit_id = $8,
deposit_finish = $9,
tx_id = $10,
paid_date = $11,
fee_paid = $12,
error = $13
WHERE id = $1
",
    )
    .bind(token_transfer.id)
    .bind(&token_transfer.payment_id)
    .bind(&token_transfer.from_addr)
    .bind(&token_transfer.receiver_addr)
    .bind(token_transfer.chain_id)
    .bind(&token_transfer.token_addr)
    .bind(&token_transfer.token_amount)
    .bind(&token_transfer.deposit_id)
    .bind(token_transfer.deposit_finish)
    .bind(token_transfer.tx_id)
    .bind(token_transfer.paid_date)
    .bind(&token_transfer.fee_paid)
    .bind(&token_transfer.error)
    .execute(executor)
    .await?;
    Ok(token_transfer.clone())
}

pub async fn get_all_token_transfers(
    conn: &SqlitePool,
    limit: Option<i64>,
) -> Result<Vec<TokenTransferDbObj>, sqlx::Error> {
    let limit = limit.unwrap_or(i64::MAX);
    let rows = sqlx::query_as::<_, TokenTransferDbObj>(
        r"SELECT * FROM token_transfer ORDER by id DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(conn)
    .await?;
    Ok(rows)
}

pub async fn get_token_transfers_by_chain_id(
    conn: &SqlitePool,
    chain_id: i64,
    limit: Option<i64>,
) -> Result<Vec<TokenTransferDbObj>, sqlx::Error> {
    let limit = limit.unwrap_or(i64::MAX);
    let rows = sqlx::query_as::<_, TokenTransferDbObj>(
        r"SELECT * FROM token_transfer WHERE chain_id = $1 ORDER by id DESC LIMIT $2",
    )
    .bind(chain_id)
    .bind(limit)
    .fetch_all(conn)
    .await?;
    Ok(rows)
}

pub async fn get_token_transfers_by_deposit_id<'c, E>(
    conn: E,
    chain_id: i64,
    deposit_id: &str,
) -> Result<Vec<TokenTransferDbObj>, sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    let rows = sqlx::query_as::<_, TokenTransferDbObj>(
        r"SELECT * FROM token_transfer WHERE chain_id = $1 AND deposit_id = $2 ORDER by id DESC",
    )
    .bind(chain_id)
    .bind(deposit_id)
    .fetch_all(conn)
    .await?;
    Ok(rows)
}

pub async fn get_pending_token_transfers(
    conn: &SqlitePool,
    account: Address,
    chain_id: i64,
) -> Result<Vec<TokenTransferDbObj>, sqlx::Error> {
    let rows = sqlx::query_as::<_, TokenTransferDbObj>(
        r"SELECT * FROM token_transfer
WHERE tx_id is null
AND error is null
AND from_addr = $1
AND chain_id = $2
ORDER by id ASC
",
    )
    .bind(format!("{:#x}", account))
    .bind(chain_id)
    .fetch_all(conn)
    .await?;
    Ok(rows)
}

pub async fn get_unpaid_token_transfers(
    conn: &SqlitePool,
    chain_id: i64,
    sender: Address,
) -> Result<Vec<TokenTransferDbObj>, sqlx::Error> {
    sqlx::query_as::<_, TokenTransferDbObj>(
        r"SELECT * FROM token_transfer
WHERE fee_paid is null
AND chain_id = $1
AND from_addr = $2
",
    )
    .bind(chain_id)
    .bind(format!("{:#x}", sender))
    .fetch_all(conn)
    .await
}

pub async fn get_token_transfers_by_tx<'c, E>(
    executor: E,
    tx_id: i64,
) -> Result<Vec<TokenTransferDbObj>, sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    let rows =
        sqlx::query_as::<_, TokenTransferDbObj>(r"SELECT * FROM token_transfer WHERE tx_id=$1")
            .bind(tx_id)
            .fetch_all(executor)
            .await?;
    Ok(rows)
}

pub const TRANSFER_FILTER_ALL: &str = "(id >= 0)";
pub const TRANSFER_FILTER_QUEUED: &str = "(tx_id is null AND error is null)";
pub const TRANSFER_FILTER_PROCESSING: &str = "(tx_id is not null AND fee_paid is null)";
pub const TRANSFER_FILTER_DONE: &str = "(fee_paid is not null)";

#[derive(Debug, Clone, Default)]
pub struct TransferStatsPart {
    pub transaction_ids: HashSet<i64>,
    pub queued_count: u64,
    pub processed_count: u64,
    pub done_count: u64,
    pub total_count: u64,
    pub fee_paid: U256,
    pub first_transfer_date: Option<DateTime<Utc>>,
    pub last_transfer_date: Option<DateTime<Utc>>,
    pub first_paid_date: Option<DateTime<Utc>>,
    pub last_paid_date: Option<DateTime<Utc>>,
    pub max_payment_delay: Option<Duration>,
    ///None means native token
    pub erc20_token_transferred: BTreeMap<Address, U256>,
    pub native_token_transferred: U256,
}

#[derive(Debug, Clone, Default)]
pub struct TransferStatsBase {
    pub per_receiver: BTreeMap<Address, TransferStatsPart>,
    pub all: TransferStatsPart,
}

#[derive(Debug, Clone, Default)]
pub struct TransferStats {
    pub per_sender: BTreeMap<Address, TransferStatsBase>,
}

pub async fn get_transfer_stats_from_blockchain(
    conn: &SqlitePool,
    chain_id: i64,
    limit: Option<i64>,
) -> Result<TransferStats, PaymentError> {
    let tt = get_chain_transfers_by_chain_id(conn, chain_id, limit)
        .await
        .map_err(err_from!())?;
    //let txs = get_transactions(conn, None, None, None)
    //    .await
    //    .map_err(err_from!())?;
    //let mut txs_map = HashMap::new();
    //for tx in txs {
    //    txs_map.insert(tx.id, tx);
    //}

    let mut ts = TransferStats::default();
    for t in tt {
        let from_addr = Address::from_str(&t.from_addr).map_err(err_from!())?;
        let to_addr = Address::from_str(&t.receiver_addr).map_err(err_from!())?;
        let ts = ts
            .per_sender
            .entry(from_addr)
            .or_insert_with(TransferStatsBase::default);
        let (t1, t2) = (
            &mut ts.all,
            ts.per_receiver
                .entry(to_addr)
                .or_insert_with(TransferStatsPart::default),
        );

        for ts in [t1, t2] {
            ts.total_count += 1;
            ts.done_count += 1;
            if let Some(fee_paid) = &t.fee_paid {
                ts.fee_paid += U256::from_dec_str(fee_paid).map_err(err_from!())?;
            }

            if let Some(paid_date) = t.blockchain_date {
                if ts.first_paid_date.is_none() || ts.first_paid_date.unwrap() > paid_date {
                    ts.first_paid_date = Some(paid_date);
                }
                if ts.last_paid_date.is_none() || ts.last_paid_date.unwrap() < paid_date {
                    ts.last_paid_date = Some(paid_date);
                }
            }
            ts.transaction_ids.insert(t.chain_tx_id);
            //ts.fee_paid += U256::from_dec_str(&t.fee_paid.clone().unwrap()).map_err(err_from!())?;
            if let Some(token_addr) = &t.token_addr {
                let token_addr = Address::from_str(token_addr).map_err(err_from!())?;
                let token_amount = U256::from_dec_str(&t.token_amount).map_err(err_from!())?;
                ts.erc20_token_transferred
                    .entry(token_addr)
                    .or_insert_with(U256::zero)
                    .add_assign(token_amount);
            } else {
                ts.native_token_transferred
                    .add_assign(U256::from_dec_str(&t.token_amount).map_err(err_from!())?);
            }
        }
    }
    Ok(ts)
}

pub async fn get_transfer_stats(
    conn: &SqlitePool,
    chain_id: i64,
    limit: Option<i64>,
) -> Result<TransferStats, PaymentError> {
    let tt = get_token_transfers_by_chain_id(conn, chain_id, limit)
        .await
        .map_err(err_from!())?;
    //let txs = get_transactions(conn, None, None, None)
    //    .await
    //    .map_err(err_from!())?;
    //let mut txs_map = HashMap::new();
    //for tx in txs {
    //    txs_map.insert(tx.id, tx);
    //}

    let mut ts = TransferStats::default();
    for t in tt {
        let from_addr = Address::from_str(&t.from_addr).map_err(err_from!())?;
        let to_addr = Address::from_str(&t.receiver_addr).map_err(err_from!())?;
        let ts = ts
            .per_sender
            .entry(from_addr)
            .or_insert_with(TransferStatsBase::default);
        let (t1, t2) = (
            &mut ts.all,
            ts.per_receiver
                .entry(to_addr)
                .or_insert_with(TransferStatsPart::default),
        );

        for ts in [t1, t2] {
            ts.total_count += 1;
            if let Some(paid_date) = t.paid_date {
                if ts.first_paid_date.is_none() || ts.first_paid_date.unwrap() > paid_date {
                    ts.first_paid_date = Some(paid_date);
                }
                if ts.last_paid_date.is_none() || ts.last_paid_date.unwrap() < paid_date {
                    ts.last_paid_date = Some(paid_date);
                }
                let duration = paid_date - t.create_date;
                if ts.max_payment_delay.is_none() || ts.max_payment_delay.unwrap() < duration {
                    ts.max_payment_delay = Some(duration);
                }
            }
            if ts.first_transfer_date.is_none() || ts.first_transfer_date.unwrap() > t.create_date {
                ts.first_transfer_date = Some(t.create_date);
            }
            if ts.last_transfer_date.is_none() || ts.last_transfer_date.unwrap() < t.create_date {
                ts.last_transfer_date = Some(t.create_date);
            }

            if let Some(tx_id) = t.tx_id {
                ts.transaction_ids.insert(tx_id);
            }
            if t.tx_id.is_none() && t.error.is_none() {
                ts.queued_count += 1;
            }
            if t.tx_id.is_some() && t.fee_paid.is_none() {
                ts.processed_count += 1;
            }
            if t.tx_id.is_some() && t.fee_paid.is_some() {
                ts.done_count += 1;
                ts.fee_paid +=
                    U256::from_dec_str(&t.fee_paid.clone().unwrap()).map_err(err_from!())?;
                if let Some(token_addr) = &t.token_addr {
                    let token_addr = Address::from_str(token_addr).map_err(err_from!())?;
                    let token_amount = U256::from_dec_str(&t.token_amount).map_err(err_from!())?;
                    ts.erc20_token_transferred
                        .entry(token_addr)
                        .or_insert_with(U256::zero)
                        .add_assign(token_amount);
                } else {
                    ts.native_token_transferred
                        .add_assign(U256::from_dec_str(&t.token_amount).map_err(err_from!())?);
                }
            }
        }
    }
    Ok(ts)
}

pub async fn get_transfer_count(
    conn: &SqlitePool,
    transfer_filter: Option<&str>,
    sender: Option<&str>,
    receiver: Option<&str>,
) -> Result<usize, sqlx::Error> {
    let transfer_filter = transfer_filter.unwrap_or(TRANSFER_FILTER_ALL);

    let count = if let Some(sender) = sender {
        sqlx::query_scalar::<_, i64>(
            format!(
                r"SELECT COUNT(*) FROM token_transfer WHERE {transfer_filter} AND from_addr = $1"
            )
            .as_str(),
        )
        .bind(sender)
        .fetch_one(conn)
        .await?
    } else if let Some(receiver) = receiver {
        sqlx::query_scalar::<_, i64>(format!(r"SELECT COUNT(*) FROM token_transfer WHERE {transfer_filter} AND receiver_addr = $1").as_str())
            .bind(receiver)
            .fetch_one(conn)
            .await?
    } else {
        sqlx::query_scalar::<_, i64>(
            format!(r"SELECT COUNT(*) FROM token_transfer WHERE {transfer_filter}").as_str(),
        )
        .fetch_one(conn)
        .await?
    };

    Ok(count as usize)
}
