use super::model::ChainTransferDbObj;
use crate::model::ChainTransferDbObjExt;
use chrono::{DateTime, Utc};
use sqlx::Executor;
use sqlx::Sqlite;
use sqlx::SqlitePool;

pub async fn insert_chain_transfer<'c, E>(
    executor: E,
    chain_transfer: &ChainTransferDbObj,
) -> Result<ChainTransferDbObj, sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    let res = sqlx::query_as::<_, ChainTransferDbObj>(
        r"INSERT INTO chain_transfer
(from_addr, receiver_addr, chain_id, token_addr, token_amount, chain_tx_id, fee_paid, blockchain_date)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING *;
",
    )
    .bind(&chain_transfer.from_addr)
    .bind(&chain_transfer.receiver_addr)
    .bind(chain_transfer.chain_id)
    .bind(&chain_transfer.token_addr)
    .bind(&chain_transfer.token_amount)
    .bind(chain_transfer.chain_tx_id)
    .bind(&chain_transfer.fee_paid)
    .bind(chain_transfer.blockchain_date)
    .fetch_one(executor)
    .await?;
    Ok(res)
}
pub async fn get_all_chain_transfers_ext(
    conn: &SqlitePool,
    chain_id: i64,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    limit: Option<i64>,
) -> Result<Vec<ChainTransferDbObjExt>, sqlx::Error> {
    let limit = limit.unwrap_or(i64::MAX);
    let rows = sqlx::query_as::<_, ChainTransferDbObjExt>(
        r"SELECT ct.*, cx.tx_hash, cx.block_number, cx.to_addr, cx.from_addr as caller_addr FROM chain_transfer as ct JOIN chain_tx as cx ON ct.chain_tx_id = cx.id WHERE ct.chain_id = $1 AND ct.blockchain_date >= $2 AND ct.blockchain_date <= $3 ORDER by id DESC LIMIT $4",
    )
        .bind(chain_id)
        .bind(from)
        .bind(to)
        .bind(limit)
        .fetch_all(conn)
        .await?;
    Ok(rows)
}

pub async fn get_all_chain_transfers_by_receiver_ext(
    conn: &SqlitePool,
    chain_id: i64,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    receiver: &str,
    limit: Option<i64>,
) -> Result<Vec<ChainTransferDbObjExt>, sqlx::Error> {
    let limit = limit.unwrap_or(i64::MAX);
    let rows = sqlx::query_as::<_, ChainTransferDbObjExt>(
        r"SELECT ct.*, cx.tx_hash, cx.block_number, cx.to_addr, cx.from_addr as caller_addr FROM chain_transfer as ct JOIN chain_tx as cx ON ct.chain_tx_id = cx.id WHERE ct.chain_id = $1 AND ct.blockchain_date >= $2 AND ct.blockchain_date <= $3 AND ct.receiver_addr = $4 ORDER by id DESC LIMIT $5",
    )
        .bind(chain_id)
        .bind(from)
        .bind(to)
        .bind(receiver)
        .bind(limit)
        .fetch_all(conn)
        .await?;
    Ok(rows)
}

pub async fn get_all_chain_transfers(
    conn: &SqlitePool,
    chain_id: i64,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    limit: Option<i64>,
) -> Result<Vec<ChainTransferDbObj>, sqlx::Error> {
    let limit = limit.unwrap_or(i64::MAX);
    let rows = sqlx::query_as::<_, ChainTransferDbObj>(
        r"SELECT * FROM chain_transfer WHERE chain_id = $1 AND blockchain_date >= $2 AND blockchain_date <= $3 ORDER by id DESC LIMIT $4",
    )
        .bind(chain_id)
    .bind(from)
    .bind(to)
    .bind(limit)
    .fetch_all(conn)
    .await?;
    Ok(rows)
}

pub async fn get_chain_transfers_by_chain_id(
    conn: &SqlitePool,
    chain_id: i64,
    limit: Option<i64>,
) -> Result<Vec<ChainTransferDbObj>, sqlx::Error> {
    let limit = limit.unwrap_or(i64::MAX);
    let rows = sqlx::query_as::<_, ChainTransferDbObj>(
        r"SELECT * FROM chain_transfer WHERE chain_id = $1 ORDER by id DESC LIMIT $2",
    )
    .bind(chain_id)
    .bind(limit)
    .fetch_all(conn)
    .await?;
    Ok(rows)
}
