use super::model::TxDbObj;
use sqlx::Sqlite;
use sqlx::SqlitePool;
use sqlx::{Executor, Transaction};
use web3::types::Address;

pub const TRANSACTION_FILTER_QUEUED: &str = "processing > 0 AND first_processed IS NULL";
pub const TRANSACTION_FILTER_PROCESSING: &str = "processing > 0 AND first_processed IS NOT NULL";
pub const TRANSACTION_FILTER_TO_PROCESS: &str = "processing > 0";
pub const TRANSACTION_FILTER_ALL: &str = "id >= 0";
pub const TRANSACTION_FILTER_DONE: &str = "processing = 0";
pub const TRANSACTION_ORDER_BY_ID_AND_REPLACEMENT_ID: &str = "orig_tx_id DESC,id ASC";
pub const TRANSACTION_ORDER_BY_CREATE_DATE: &str = "created_date ASC";
pub const TRANSACTION_ORDER_BY_FIRST_PROCESSED_DATE_DESC: &str = "first_processed DESC";

pub async fn get_next_transaction<'c, E>(
    executor: E,
    chain_id: i64,
    account: &str,
) -> Result<Option<TxDbObj>, sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    let res = sqlx::query_as::<_, TxDbObj>(
        r"SELECT * FROM tx
        WHERE chain_id = $1 AND from_addr = $2 AND processing > 0 AND first_processed IS NULL
        ORDER BY id ASC LIMIT 1",
    )
    .bind(chain_id)
    .bind(account)
    .fetch_optional(executor)
    .await?;
    Ok(res)
}

pub async fn get_transactions<'c, E>(
    executor: E,
    account: Option<Address>,
    filter: Option<&str>,
    limit: Option<i64>,
    order: Option<&str>,
    chain_id: Option<i64>,
) -> Result<Vec<TxDbObj>, sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    let limit = limit.unwrap_or(i64::MAX);
    let filter = filter.unwrap_or(TRANSACTION_FILTER_ALL);
    let order = order.unwrap_or("id DESC");
    let filter_account = match account {
        Some(addr) => format!("from_addr = '{:#x}'", addr),
        None => "1 = 1".to_string(),
    };
    let filter_chain = match chain_id {
        Some(chain_id) => format!("chain_id = {}", chain_id),
        None => "1 = 1".to_string(),
    };
    let rows = sqlx::query_as::<_, TxDbObj>(
        format!(r"SELECT * FROM tx WHERE ({filter_chain}) AND ({filter_account}) AND ({filter}) ORDER BY {order} LIMIT {limit}").as_str(),
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

pub async fn get_transaction<'c, E>(executor: E, tx_id: i64) -> Result<TxDbObj, sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    let row = sqlx::query_as::<_, TxDbObj>(r"SELECT * FROM tx WHERE id = $1")
        .bind(tx_id)
        .fetch_one(executor)
        .await?;
    Ok(row)
}

pub async fn get_last_unsent_tx<'c, E>(
    executor: E,
    tx_id: i64,
) -> Result<Option<TxDbObj>, sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    let row = sqlx::query_as::<_, TxDbObj>(r"SELECT * FROM tx WHERE broadcast_date is NULL AND signed_date is NULL ORDER BY id DESC LIMIT 1")
        .bind(tx_id)
        .fetch_optional(executor)
        .await?;
    Ok(row)
}

//call in transaction
pub async fn get_transaction_chain(
    executor: &mut Transaction<'_, Sqlite>,
    tx_id: i64,
) -> Result<Vec<TxDbObj>, sqlx::Error> {
    let mut current_id = Some(tx_id);
    let mut res = vec![];
    while let Some(id) = current_id {
        let row = sqlx::query_as::<_, TxDbObj>(r"SELECT * FROM tx WHERE id = $1")
            .bind(id)
            .fetch_one(&mut **executor)
            .await?;
        current_id = row.orig_tx_id;
        res.push(row);
    }
    Ok(res)
}

pub async fn delete_tx<'c, E>(executor: E, tx_id: i64) -> Result<(), sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    sqlx::query(r"DELETE FROM tx WHERE id = $1")
        .bind(tx_id)
        .execute(executor)
        .await?;
    Ok(())
}

pub async fn get_transaction_highest_block(
    conn: &SqlitePool,
    chain_id: i64,
    from_addr: &str,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar::<_, Option<i64>>(
        r"SELECT MAX(block_number) FROM tx WHERE confirm_date
         IS NOT NULL
         AND chain_id = $1
         AND from_addr = $2
         ",
    )
    .bind(chain_id)
    .bind(from_addr)
    .fetch_one(conn)
    .await
}

pub async fn get_transaction_highest_nonce(
    conn: &SqlitePool,
    chain_id: i64,
    from_addr: &str,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar::<_, Option<i64>>(
        r"SELECT MAX(nonce) FROM tx WHERE confirm_date
         IS NOT NULL
         AND chain_id = $1
         AND from_addr = $2
         ",
    )
    .bind(chain_id)
    .bind(from_addr)
    .fetch_one(conn)
    .await
}

pub async fn get_transaction_count(
    conn: &SqlitePool,
    transaction_filter: Option<&str>,
) -> Result<usize, sqlx::Error> {
    let transaction_filter = transaction_filter.unwrap_or(TRANSACTION_FILTER_ALL);
    let count = sqlx::query_scalar::<_, i64>(
        format!(r"SELECT COUNT(*) FROM tx WHERE {transaction_filter}").as_str(),
    )
    .fetch_one(conn)
    .await?;
    Ok(count as usize)
}

pub async fn get_next_transactions_to_process(
    conn: &SqlitePool,
    account: Option<Address>,
    limit: i64,
    chain_id: i64,
) -> Result<Vec<TxDbObj>, sqlx::Error> {
    get_transactions(
        conn,
        account,
        Some(TRANSACTION_FILTER_TO_PROCESS),
        Some(limit),
        Some(TRANSACTION_ORDER_BY_ID_AND_REPLACEMENT_ID),
        Some(chain_id),
    )
    .await
}

pub async fn force_tx_error(conn: &SqlitePool, tx: &TxDbObj) -> Result<(), sqlx::Error> {
    sqlx::query(r"UPDATE tx SET error = 'forced error' WHERE id = $1")
        .bind(tx.id)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn insert_tx<'c, E>(executor: E, tx: &TxDbObj) -> Result<TxDbObj, sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    let res = sqlx::query_as::<_, TxDbObj>(
        r"INSERT INTO tx
(method, from_addr, to_addr, chain_id, gas_limit, max_fee_per_gas, priority_fee, val, nonce, processing, call_data, created_date, first_processed, tx_hash, signed_raw_data, signed_date, broadcast_date, broadcast_count, first_stuck_date, confirm_date, blockchain_date, gas_used, block_number, chain_status, block_gas_price, effective_gas_price, fee_paid, error, orig_tx_id)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29) RETURNING *;
",
    )
        .bind(&tx.method)
        .bind(&tx.from_addr)
        .bind(&tx.to_addr)
        .bind( tx.chain_id)
        .bind( tx.gas_limit)
        .bind( &tx.max_fee_per_gas)
        .bind( &tx.priority_fee)
        .bind( &tx.val)
        .bind( tx.nonce)
        .bind( tx.processing)
        .bind( &tx.call_data)
        .bind( tx.created_date)
        .bind( tx.first_processed)
        .bind( &tx.tx_hash)
        .bind( &tx.signed_raw_data)
        .bind( tx.signed_date)
        .bind( tx.broadcast_date)
        .bind( tx.broadcast_count)
        .bind( tx.first_stuck_date)
        .bind( tx.confirm_date)
        .bind( tx.blockchain_date)
        .bind( tx.gas_used)
        .bind( tx.block_number)
        .bind( tx.chain_status)
        .bind( &tx.block_gas_price)
        .bind( &tx.effective_gas_price)
        .bind( &tx.fee_paid)
        .bind(&tx.error)
        .bind( tx.orig_tx_id)
        .fetch_one(executor)
        .await?;
    Ok(res)
}

pub async fn update_processing_and_first_processed_tx<'c, E>(
    executor: E,
    tx: &TxDbObj,
) -> Result<TxDbObj, sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    let _res = sqlx::query(
        r"UPDATE tx SET
processing = $2,
first_processed = $3
WHERE id = $1
",
    )
    .bind(tx.id)
    .bind(tx.processing)
    .bind(tx.first_processed)
    .execute(executor)
    .await?;
    Ok(tx.clone())
}

pub async fn update_tx<'c, E>(executor: E, tx: &TxDbObj) -> Result<TxDbObj, sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    let _res = sqlx::query(
        r"UPDATE tx SET
method = $2,
from_addr = $3,
to_addr = $4,
chain_id = $5,
gas_limit = $6,
max_fee_per_gas = $7,
priority_fee = $8,
val = $9,
nonce = $10,
processing = $11,
call_data = $12,
created_date = $13,
first_processed = $14,
tx_hash = $15,
signed_raw_data = $16,
signed_date = $17,
broadcast_date = $18,
broadcast_count = $19,
first_stuck_date = $20,
confirm_date = $21,
blockchain_date = $22,
gas_used = $23,
block_number = $24,
chain_status = $25,
block_gas_price = $26,
effective_gas_price = $27,
fee_paid = $28,
error = $29,
orig_tx_id = $30
WHERE id = $1
",
    )
    .bind(tx.id)
    .bind(&tx.method)
    .bind(&tx.from_addr)
    .bind(&tx.to_addr)
    .bind(tx.chain_id)
    .bind(tx.gas_limit)
    .bind(&tx.max_fee_per_gas)
    .bind(&tx.priority_fee)
    .bind(&tx.val)
    .bind(tx.nonce)
    .bind(tx.processing)
    .bind(&tx.call_data)
    .bind(tx.created_date)
    .bind(tx.first_processed)
    .bind(&tx.tx_hash)
    .bind(&tx.signed_raw_data)
    .bind(tx.signed_date)
    .bind(tx.broadcast_date)
    .bind(tx.broadcast_count)
    .bind(tx.first_stuck_date)
    .bind(tx.confirm_date)
    .bind(tx.blockchain_date)
    .bind(tx.gas_used)
    .bind(tx.block_number)
    .bind(tx.chain_status)
    .bind(&tx.block_gas_price)
    .bind(&tx.effective_gas_price)
    .bind(&tx.fee_paid)
    .bind(&tx.error)
    .bind(tx.orig_tx_id)
    .execute(executor)
    .await?;
    Ok(tx.clone())
}

pub async fn update_tx_stuck_date<'c, E>(executor: E, tx: &TxDbObj) -> Result<TxDbObj, sqlx::Error>
where
    E: Executor<'c, Database = Sqlite>,
{
    let _res = sqlx::query(
        r"UPDATE tx SET
first_stuck_date = $2
WHERE id = $1
",
    )
    .bind(tx.id)
    .bind(tx.first_stuck_date)
    .execute(executor)
    .await?;
    Ok(tx.clone())
}

#[tokio::test]
async fn tx_test() -> sqlx::Result<()> {
    println!("Start tx_test...");

    use crate::create_sqlite_connection;
    let conn = create_sqlite_connection(None, None, false, true)
        .await
        .unwrap();

    println!("In memory DB created");

    let mut tx_to_insert = TxDbObj {
        id: -1,
        tx_hash: Some(
            "0x13d8a54dec1c0a30f1cd5129f690c3e27b9aadd59504957bad4d247966dadae7".to_string(),
        ),
        signed_raw_data: None,
        signed_date: Some(chrono::Utc::now()),
        broadcast_date: Some(chrono::Utc::now()),
        broadcast_count: 45,
        first_stuck_date: Some(chrono::Utc::now()),
        method: "".to_string(),
        from_addr: "0x001066290077e38f222cc6009c0c7a91d5192303".to_string(),
        to_addr: "0xbcfe9736a4f5bf2e43620061ff3001ea0d003c0f".to_string(),
        chain_id: 987789,
        gas_limit: Some(100000),
        max_fee_per_gas: Some("110000000000".to_string()),
        priority_fee: Some("5110000000000".to_string()),
        val: "0".to_string(),
        nonce: Some(1),
        processing: 0,
        call_data: None,
        created_date: chrono::Utc::now(),
        block_number: Some(119677),
        chain_status: Some(1),
        block_gas_price: Some("557034000005500".to_string()),
        effective_gas_price: Some("103434000005500".to_string()),
        fee_paid: Some("83779300533141".to_string()),
        error: Some("Test error message".to_string()),
        orig_tx_id: None,
        engine_message: None,
        engine_error: None,
        first_processed: None,
        confirm_date: Some(chrono::Utc::now()),
        blockchain_date: Some(chrono::Utc::now()),
        gas_used: Some(40000),
    };

    let tx_from_insert = insert_tx(&conn, &tx_to_insert).await?;
    tx_to_insert.id = tx_from_insert.id;
    let tx_from_dao = get_transaction(&conn, tx_from_insert.id).await?;

    //all three should be equal
    assert_eq!(tx_to_insert, tx_from_dao);
    assert_eq!(tx_from_insert, tx_from_dao);

    let mut tx_update = tx_from_dao.clone();
    tx_update.first_stuck_date = None;
    update_tx_stuck_date(&conn, &tx_update).await?;
    let tx_from_dao = get_transaction(&conn, tx_from_insert.id).await?;
    assert_eq!(tx_update, tx_from_dao);

    Ok(())
}
