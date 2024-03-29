use super::model::ChainTxDbObj;
use chrono::{DateTime, Utc};
use sqlx::Executor;
use sqlx::Sqlite;
use sqlx::SqlitePool;

pub async fn insert_chain_tx<'c, E>(
    executor: E,
    tx: &ChainTxDbObj,
) -> Result<ChainTxDbObj, sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    let res = sqlx::query_as::<_, ChainTxDbObj>(
        r"INSERT INTO chain_tx
(tx_hash, method, from_addr, to_addr, chain_id, gas_used, gas_limit, block_gas_price, effective_gas_price, max_fee_per_gas, priority_fee, val, nonce, checked_date, blockchain_date, block_number, chain_status, fee_paid, error, balance_eth, balance_glm)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21) RETURNING *",
    )
    .bind(&tx.tx_hash)
    .bind(&tx.method)
    .bind(&tx.from_addr)
    .bind(&tx.to_addr)
    .bind(tx.chain_id)
    .bind(tx.gas_used)
    .bind(tx.gas_limit)
    .bind(&tx.block_gas_price)
    .bind(&tx.effective_gas_price)
    .bind(&tx.max_fee_per_gas)
    .bind(&tx.priority_fee)
    .bind(&tx.val)
    .bind(tx.nonce)
    .bind(tx.checked_date)
    .bind(tx.blockchain_date)
    .bind(tx.block_number)
    .bind(tx.chain_status)
    .bind(&tx.fee_paid)
    .bind(&tx.error)
    .bind(&tx.balance_eth)
    .bind(&tx.balance_glm)
    .fetch_one(executor)
    .await?;
    Ok(res)
}

pub async fn get_chain_txs_by_chain_id(
    conn: &SqlitePool,
    chain_id: i64,
    limit: Option<i64>,
) -> Result<Vec<ChainTxDbObj>, sqlx::Error> {
    let limit = limit.unwrap_or(i64::MAX);
    let rows = sqlx::query_as::<_, ChainTxDbObj>(
        r"SELECT * FROM chain_tx WHERE chain_id = $1 ORDER by id DESC LIMIT $2",
    )
    .bind(chain_id)
    .bind(limit)
    .fetch_all(conn)
    .await?;
    Ok(rows)
}

pub async fn get_chain_txs_by_chain_id_and_dates(
    conn: &SqlitePool,
    chain_id: i64,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    limit: Option<i64>,
) -> Result<Vec<ChainTxDbObj>, sqlx::Error> {
    let limit = limit.unwrap_or(i64::MAX);
    let rows = sqlx::query_as::<_, ChainTxDbObj>(
        r"SELECT * FROM chain_tx WHERE chain_id = $1 AND blockchain_date >= $2 AND blockchain_date <= $3 ORDER by id DESC LIMIT $4",
    )
    .bind(chain_id)
    .bind(from)
    .bind(to)
    .bind(limit)
    .fetch_all(conn)
    .await?;
    Ok(rows)
}

pub async fn get_chain_tx(conn: &SqlitePool, id: i64) -> Result<ChainTxDbObj, sqlx::Error> {
    let row = sqlx::query_as::<_, ChainTxDbObj>(r"SELECT * FROM chain_tx WHERE id = $1")
        .bind(id)
        .fetch_one(conn)
        .await?;
    Ok(row)
}

pub async fn get_chain_tx_hash<'c, E>(
    executor: E,
    tx_hash: String,
) -> Result<Option<ChainTxDbObj>, sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    let row = sqlx::query_as::<_, ChainTxDbObj>(r"SELECT * FROM chain_tx WHERE tx_hash = $1")
        .bind(tx_hash)
        .fetch_optional(executor)
        .await?;
    Ok(row)
}

pub async fn get_last_scanned_block<'c, E>(
    executor: E,
    chain_id: i64,
) -> Result<Option<i64>, sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    sqlx::query_scalar::<_, i64>(r"SELECT MAX(block_number) FROM chain_tx WHERE chain_id = $1")
        .bind(chain_id)
        .fetch_optional(executor)
        .await
}

#[tokio::test]
async fn tx_chain_test() -> sqlx::Result<()> {
    println!("Start tx_chain_test...");

    use crate::create_sqlite_connection;
    let conn = create_sqlite_connection(None, None, false, true)
        .await
        .unwrap();

    println!("In memory DB created");

    let mut tx_to_insert = ChainTxDbObj {
        id: -1,
        tx_hash: "0x13d8a54dec1c0a30f1cd5129f690c3e27b9aadd59504957bad4d247966dadae7".to_string(),
        method: "".to_string(),
        from_addr: "0x001066290077e38f222cc6009c0c7a91d5192303".to_string(),
        to_addr: "0xbcfe9736a4f5bf2e43620061ff3001ea0d003c0f".to_string(),
        block_gas_price: Some("6103434000044".to_string()),
        effective_gas_price: Some("103434000000000".to_string()),
        chain_id: 987789,
        gas_used: Some(40000),
        gas_limit: Some(100000),
        max_fee_per_gas: Some("110000000000".to_string()),
        priority_fee: Some("5110000000000".to_string()),
        val: "0".to_string(),
        nonce: 1,
        checked_date: chrono::Utc::now(),
        blockchain_date: chrono::Utc::now(),
        block_number: 119677,
        chain_status: 1,
        fee_paid: "83779300533141".to_string(),
        error: Some("Test error message".to_string()),
        engine_message: None,
        engine_error: None,
        balance_eth: Some("4".to_string()),
        balance_glm: Some("5".to_string()),
    };

    let tx_from_insert = insert_chain_tx(&conn, &tx_to_insert).await?;
    tx_to_insert.id = tx_from_insert.id;
    let tx_from_dao = get_chain_tx(&conn, tx_from_insert.id).await?;

    //all three should be equal
    assert_eq!(tx_to_insert, tx_from_dao);
    assert_eq!(tx_from_insert, tx_from_dao);

    Ok(())
}
