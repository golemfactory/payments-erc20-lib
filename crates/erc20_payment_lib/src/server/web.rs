use crate::eth::{
    get_attestation_details, get_balance, get_schema_details, Attestation, AttestationSchema,
    GetBalanceArgs,
};
use crate::runtime::{PaymentRuntime, SharedState, TransferArgs, TransferType};
use crate::server::ws::event_stream_websocket_endpoint;
use crate::setup::{ChainSetup, PaymentSetup};
use crate::transaction::create_token_transfer;
use actix_files::NamedFile;
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::error::ErrorBadRequest;
use actix_web::http::header::HeaderValue;
use actix_web::http::{header, StatusCode};
use actix_web::web::Data;
use actix_web::{web, HttpRequest, HttpResponse, Responder, Scope};
use chrono::{DateTime, Utc};
use erc20_payment_lib_common::model::DepositId;
use erc20_payment_lib_common::ops::*;
use erc20_payment_lib_common::{export_metrics_to_prometheus, FaucetData};
use erc20_rpc_pool::VerifyEndpointResult;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::SqlitePool;
use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use web3::ethabi;
use web3::types::{Address, H256, U256};

pub struct ServerData {
    pub shared_state: Arc<std::sync::Mutex<SharedState>>,
    pub db_connection: Arc<Mutex<SqlitePool>>,
    pub payment_setup: PaymentSetup,
    pub payment_runtime: PaymentRuntime,
}

macro_rules! return_on_error {
    ( $e:expr ) => {
        match $e {
            Ok(x) => x,
            Err(err) => {
                return web::Json(json!({
                    "error": err.to_string()
                }));
            }
        }
    }
}

pub async fn tx_details(data: Data<Box<ServerData>>, req: HttpRequest) -> impl Responder {
    let tx_id = req
        .match_info()
        .get("tx_id")
        .map(|tx_id| i64::from_str(tx_id).ok())
        .unwrap_or(None);

    let tx_id = match tx_id {
        Some(tx_id) => tx_id,
        None => return web::Json(json!({"error": "failed to parse tx_id"})),
    };

    let tx = {
        let db_conn = data.db_connection.lock().await;
        match get_transaction(&*db_conn, tx_id).await {
            Ok(allowances) => allowances,
            Err(err) => {
                return web::Json(json!({
                    "error": err.to_string()
                }));
                //return format!("Error getting allowances: {:?}", err);
            }
        }
    };

    /*
    let transfers = {
        let db_conn = data.db_connection.lock().await;
        match get_token_transfers_by_tx(&db_conn, tx_id).await {
            Ok(allowances) => allowances,
            Err(err) => {
                return web::Json(json!({
                    "error": err.to_string()
                }))
            }
        }
    };*/
    /*let json_transfers = transfers
    .iter()
    .map(|transfer| {
        json!({
            "id": transfer.id,
            "chain_id": transfer.chain_id,
            "tx_id": transfer.tx_id,
            "from": transfer.from_addr,
            "receiver": transfer.receiver_addr,
            "token": transfer.token_addr,
            "amount": transfer.token_amount,
            "fee_paid": transfer.fee_paid,
        })
    })
    .collect::<Vec<_>>();*/

    web::Json(json!({
        "tx": tx,
    }))
}

pub async fn rpc_pool(data: Data<Box<ServerData>>, _req: HttpRequest) -> impl Responder {
    let my_data = data.shared_state.lock().unwrap();
    //synchronize rpc_pool statistics with server
    /*shared_state.lock().await.web3_rpc_pool.insert(
        chain_id,
        web3.endpoints
            .iter()
            .map(|e| {
                (
                    e.read().unwrap().web3_rpc_params.clone(),
                    e.read().unwrap().web3_rpc_info.clone(),
                )
            })
            .collect::<Vec<(Web3RpcParams, Web3RpcInfo)>>(),
    );*/
    // Convert BTreeMap of Arenas to BTreeMap of Vec because serde can't serialize Arena
    let web3_rpc_pool_info = my_data
        .web3_pool_ref
        .lock()
        .unwrap()
        .iter()
        .map(|(k, v)| {
            (
                *k,
                v.try_lock_for(Duration::from_secs(5))
                    .unwrap()
                    .iter()
                    .map(|pair| pair.1.clone())
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut array = Vec::with_capacity(web3_rpc_pool_info.len());

    for (idx, val) in web3_rpc_pool_info {
        let val = val
            .iter()
            .map(|v| json!(*v.try_read_for(Duration::from_secs(5)).unwrap()))
            .collect::<Vec<_>>();
        let chain_network = data
            .payment_setup
            .chain_setup
            .get(&idx)
            .map(|s| s.network.clone())
            .unwrap_or("unknown".to_string());
        array.push(json!(
            {
                "chainId": idx,
                "chainNetwork": chain_network,
                "endpoints": val,
            }
        ));
    }
    web::Json(json!({
        "networks": array,
    }))
}

struct MetricGroup {
    metric_help: String,
    metric_type: String,
    metrics: Vec<Metric>,
}

struct Metric {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

pub async fn rpc_pool_metrics(data: Data<Box<ServerData>>, _req: HttpRequest) -> impl Responder {
    let pool_ref = data
        .shared_state
        .lock()
        .unwrap()
        .web3_pool_ref
        .lock()
        .unwrap()
        .clone();

    let mut metrics = Vec::with_capacity(100);

    metrics.push(MetricGroup {
        metric_help: "# HELP rpc_endpoint_effective_score Effective score of selected rpc endpoint"
            .to_string(),
        metric_type: "# TYPE rpc_endpoint_effective_score gauge".to_string(),
        metrics: Vec::new(),
    });
    metrics.push(MetricGroup {
        metric_help:
            "# HELP rpc_endpoint_score_validation Score (from validation) of selected rpc endpoint"
                .to_string(),
        metric_type: "# TYPE rpc_endpoint_score_validation gauge".to_string(),
        metrics: Vec::new(),
    });
    metrics.push(MetricGroup {
        metric_help: "# HELP rpc_endpoint_error_count Number of error requests".to_string(),
        metric_type: "# TYPE rpc_endpoint_error_count counter".to_string(),
        metrics: Vec::new(),
    });
    metrics.push(MetricGroup {
        metric_help: "# HELP rpc_endpoint_success_count Number of succeeded requests".to_string(),
        metric_type: "# TYPE rpc_endpoint_success_count counter".to_string(),
        metrics: Vec::new(),
    });
    metrics.push(MetricGroup {
        metric_help: "# HELP rpc_endpoint_ms Endpoint validation time".to_string(),
        metric_type: "# TYPE rpc_endpoint_ms gauge".to_string(),
        metrics: Vec::new(),
    });
    metrics.push(MetricGroup {
        metric_help: "# HELP rpc_endpoint_block_delay Time since last block head".to_string(),
        metric_type: "# TYPE rpc_endpoint_block_delay gauge".to_string(),
        metrics: Vec::new(),
    });

    for (_idx, vec) in pool_ref {
        for (_idx, endpoint) in vec.try_lock_for(Duration::from_secs(5)).unwrap().iter() {
            let endpoint = endpoint
                .try_read_for(std::time::Duration::from_secs(5))
                .unwrap();
            let params = vec![
                (
                    "chain_id".to_string(),
                    endpoint.web3_rpc_params.chain_id.to_string(),
                ),
                ("name".to_string(), endpoint.web3_rpc_params.name.clone()),
            ];
            let new_metric = Metric {
                name: "rpc_endpoint_effective_score".into(),
                params: params.clone(),
                value: (endpoint.get_score()).to_string(),
            };
            metrics[0].metrics.push(new_metric);

            let new_metric = Metric {
                name: "rpc_endpoint_score_validation".into(),
                params: params.clone(),
                value: (endpoint.get_validation_score()).to_string(),
            };
            metrics[1].metrics.push(new_metric);

            let new_metric = Metric {
                name: "rpc_endpoint_error_count".into(),
                params: params.clone(),
                value: endpoint
                    .web3_rpc_info
                    .web3_rpc_stats
                    .request_count_total_error
                    .to_string(),
            };
            metrics[2].metrics.push(new_metric);

            let new_metric = Metric {
                name: "rpc_endpoint_success_count".into(),
                params: params.clone(),
                value: endpoint
                    .web3_rpc_info
                    .web3_rpc_stats
                    .request_count_total_succeeded
                    .to_string(),
            };
            metrics[3].metrics.push(new_metric);

            let (head_behind, check_time_ms) = match &endpoint.web3_rpc_info.verify_result {
                Some(VerifyEndpointResult::Ok(res)) => {
                    (res.head_seconds_behind as i64, res.check_time_ms as i64)
                }
                _ => (-1, -1),
            };

            let new_metric = Metric {
                name: "rpc_endpoint_ms".into(),
                params: params.clone(),
                value: check_time_ms.to_string(),
            };
            metrics[4].metrics.push(new_metric);

            let new_metric = Metric {
                name: "rpc_endpoint_block_delay".into(),
                params: params.clone(),
                value: head_behind.to_string(),
            };
            metrics[5].metrics.push(new_metric);
        }
    }

    let mut resp: String = String::with_capacity(1024 * 1024);
    for metric_group in metrics {
        resp += &format!("{}\n", metric_group.metric_help);
        resp += &format!("{}\n", metric_group.metric_type);
        for metric in metric_group.metrics {
            resp += &format!("{}{{", metric.name);
            for (idx, param) in metric.params.iter().enumerate() {
                resp += &format!(
                    "{}=\"{}\"{}",
                    param.0,
                    param.1,
                    if idx < metric.params.len() - 1 {
                        ","
                    } else {
                        ""
                    }
                );
            }
            resp += &format!("}} {}\n", metric.value);
        }
        resp += "\n";
    }

    resp
}

pub async fn allowances(data: Data<Box<ServerData>>, _req: HttpRequest) -> impl Responder {
    data.shared_state.lock().unwrap().inserted += 1;
    let allowances = {
        let db_conn = data.db_connection.lock().await;
        match get_all_allowances(&db_conn).await {
            Ok(allowances) => allowances,
            Err(err) => {
                return web::Json(json!({
                    "error": err.to_string()
                }));
                //return format!("Error getting allowances: {:?}", err);
            }
        }
    };

    web::Json(json!({
        "allowances": allowances,
    }))
}

pub async fn transactions_count(data: Data<Box<ServerData>>, _req: HttpRequest) -> impl Responder {
    let queued_tx_count = {
        let db_conn = data.db_connection.lock().await;
        return_on_error!(get_transaction_count(&db_conn, Some(TRANSACTION_FILTER_QUEUED)).await)
    };
    let done_tx_count = {
        let db_conn = data.db_connection.lock().await;
        return_on_error!(get_transaction_count(&db_conn, Some(TRANSACTION_FILTER_DONE)).await)
    };

    let queued_transfer_count = {
        let db_conn = data.db_connection.lock().await;
        return_on_error!(
            get_transfer_count(&db_conn, Some(TRANSFER_FILTER_QUEUED), None, None).await
        )
    };
    let processed_transfer_count = {
        let db_conn = data.db_connection.lock().await;
        return_on_error!(
            get_transfer_count(&db_conn, Some(TRANSFER_FILTER_PROCESSING), None, None).await
        )
    };
    let done_transfer_count = {
        let db_conn = data.db_connection.lock().await;
        return_on_error!(get_transfer_count(&db_conn, Some(TRANSFER_FILTER_DONE), None, None).await)
    };

    web::Json(json!({
        "transfersQueued": queued_transfer_count,
        "transfersProcessing": processed_transfer_count,
        "transfersDone": done_transfer_count,
        "txQueued": queued_tx_count,
        "txDone": done_tx_count,
    }))
}

pub async fn config_endpoint(data: Data<Box<ServerData>>) -> impl Responder {
    let payment_setup = data.payment_setup.clone();

    web::Json(json!({
        "config": payment_setup,
    }))
}

pub async fn debug_endpoint(data: Data<Box<ServerData>>) -> impl Responder {
    let shared_state = data.shared_state.lock().unwrap().clone();

    web::Json(json!({
        "sharedState": shared_state,
    }))
}

pub async fn transactions(data: Data<Box<ServerData>>, _req: HttpRequest) -> impl Responder {
    //todo: add limits
    let txs = {
        let db_conn = data.db_connection.lock().await;
        return_on_error!(get_transactions(&*db_conn, None, None, None, None, None).await)
    };
    web::Json(json!({
        "txs": txs,
    }))
}

pub async fn skip_pending_operation(
    data: Data<Box<ServerData>>,
    req: HttpRequest,
) -> impl Responder {
    let tx_id = req
        .match_info()
        .get("tx_id")
        .map(|tx_id| i64::from_str(tx_id).ok())
        .unwrap_or(None);
    if let Some(tx_id) = tx_id {
        if data.shared_state.lock().unwrap().skip_tx(tx_id) {
            web::Json(json!({
                "success": "true",
            }))
        } else {
            web::Json(json!({
                "error": "Tx not found",
            }))
        }
    } else {
        web::Json(json!({
            "error": "failed to parse tx_id",
        }))
    }
}

pub async fn transactions_next(data: Data<Box<ServerData>>, req: HttpRequest) -> impl Responder {
    let limit = req
        .match_info()
        .get("count")
        .map(|tx_id| i64::from_str(tx_id).ok())
        .unwrap_or(Some(10));

    let txs = {
        let db_conn = data.db_connection.lock().await;
        return_on_error!(
            get_transactions(
                &*db_conn,
                None,
                Some(TRANSACTION_FILTER_QUEUED),
                limit,
                Some(TRANSACTION_ORDER_BY_CREATE_DATE),
                None
            )
            .await
        )
    };
    web::Json(json!({
        "txs": txs,
    }))
}

pub async fn transactions_current(
    data: Data<Box<ServerData>>,
    _req: HttpRequest,
) -> impl Responder {
    let txs = {
        let db_conn = data.db_connection.lock().await;
        return_on_error!(
            get_transactions(
                &*db_conn,
                None,
                Some(TRANSACTION_FILTER_PROCESSING),
                None,
                Some(TRANSACTION_ORDER_BY_CREATE_DATE),
                None
            )
            .await
        )
    };
    web::Json(json!({
        "txs": txs,
    }))
}

pub async fn transactions_last_processed(
    data: Data<Box<ServerData>>,
    req: HttpRequest,
) -> impl Responder {
    let limit = req
        .match_info()
        .get("count")
        .map(|tx_id| i64::from_str(tx_id).ok())
        .unwrap_or(Some(10));

    let txs = {
        let db_conn = data.db_connection.lock().await;
        return_on_error!(
            get_transactions(
                &*db_conn,
                None,
                Some(TRANSACTION_FILTER_DONE),
                limit,
                Some(TRANSACTION_ORDER_BY_FIRST_PROCESSED_DATE_DESC),
                None
            )
            .await
        )
    };
    web::Json(json!({
        "txs": txs,
    }))
}

pub async fn transactions_feed(data: Data<Box<ServerData>>, req: HttpRequest) -> impl Responder {
    let limit_prev = req
        .match_info()
        .get("prev")
        .map(|tx_id| i64::from_str(tx_id).ok())
        .unwrap_or(Some(10));
    let limit_next = req
        .match_info()
        .get("next")
        .map(|tx_id| i64::from_str(tx_id).ok())
        .unwrap_or(Some(10));
    let mut txs = {
        let db_conn = data.db_connection.lock().await;
        let mut db_transaction = return_on_error!(db_conn.begin().await);
        let mut txs = return_on_error!(
            get_transactions(
                &mut *db_transaction,
                None,
                Some(TRANSACTION_FILTER_DONE),
                limit_prev,
                Some(TRANSACTION_ORDER_BY_FIRST_PROCESSED_DATE_DESC),
                None
            )
            .await
        );
        let txs_current = return_on_error!(
            get_transactions(
                &mut *db_transaction,
                None,
                Some(TRANSACTION_FILTER_PROCESSING),
                None,
                Some(TRANSACTION_ORDER_BY_CREATE_DATE),
                None
            )
            .await
        );
        let tx_next = return_on_error!(
            get_transactions(
                &mut *db_transaction,
                None,
                Some(TRANSACTION_FILTER_QUEUED),
                limit_next,
                Some(TRANSACTION_ORDER_BY_CREATE_DATE),
                None
            )
            .await
        );
        return_on_error!(db_transaction.commit().await);
        //join transactions
        txs.reverse();
        txs.extend(txs_current);
        txs.extend(tx_next);
        txs
    };

    let current_tx = data.shared_state.lock().unwrap().current_tx_info.clone();
    for tx in txs.iter_mut() {
        if let Some(tx_info) = current_tx.get(&tx.id) {
            tx.engine_error.clone_from(&tx_info.error);
            tx.engine_message = Some(tx_info.message.clone());
        }
    }

    web::Json(json!({
        "txs": txs,
        "current": current_tx,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransactionRequest {
    from: String,
    to: String,
    token: Option<String>,
    amount: String,
    chain: i64,
    due_date: Option<String>,
    payment_id: Option<String>,
    deposit_id: Option<DepositId>,
}

async fn new_transfer(
    data: Data<Box<ServerData>>,
    _req: HttpRequest,
    new_transfer: web::Json<TransactionRequest>,
) -> actix_web::Result<String> {
    //println!("new_transfer: {:?}", new_transfer);

    let chain = data
        .payment_setup
        .chain_setup
        .get(&new_transfer.chain)
        .ok_or(actix_web::error::ErrorBadRequest("No config found"))?
        .clone();

    let tx_type = if let Some(_token) = &new_transfer.token {
        TransferType::Token
    } else {
        TransferType::Gas
    };

    let due_date = if let Some(due_date) = &new_transfer.due_date {
        Some(
            chrono::DateTime::parse_from_rfc3339(due_date)
                .map_err(|err| {
                    actix_web::error::ErrorBadRequest(format!("Invalid due_date: {}", err))
                })?
                .naive_utc()
                .and_utc(),
        )
    } else {
        None
    };

    let payment_id = if let Some(payment_id) = &new_transfer.payment_id {
        payment_id.clone()
    } else {
        uuid::Uuid::new_v4().to_string()
    };

    let transfer_args = TransferArgs {
        network: chain.network,
        from: Address::from_str(&new_transfer.from).unwrap(),
        receiver: Address::from_str(&new_transfer.to).unwrap(),
        tx_type,
        amount: U256::from_dec_str(&new_transfer.amount).unwrap(),
        payment_id,
        deadline: due_date,
        deposit_id: new_transfer.deposit_id,
    };

    let account = match data
        .shared_state
        .lock()
        .unwrap()
        .accounts
        .iter()
        .find(|acc| acc.address == transfer_args.from)
    {
        Some(acc) => acc.clone(),
        None => {
            return Err(actix_web::error::ErrorBadRequest(format!(
                "Account not found: {:#x}",
                transfer_args.from
            )));
        }
    };

    if let Err(err) = data
        .payment_runtime
        .transfer_with_account(&account, transfer_args.clone())
        .await
    {
        return Err(actix_web::error::ErrorInternalServerError(format!(
            "Failed to create transfer: {}",
            err
        )));
    };
    log::warn!("Created transfer: {:?}", transfer_args);

    Ok("success".to_string())
}

#[derive(Deserialize)]
pub struct StatsTransferRequest {
    receiver: Option<String>,
    from: Option<String>,
    to: Option<String>,
    chain: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StatsTransferResult {
    request_time: f64,
    transfers: Vec<ChainTransferRespObj>,
}

#[derive(Serialize, sqlx::FromRow, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChainTransferRespObj {
    pub id: i64,
    pub from_addr: String,
    pub receiver_addr: String,
    pub chain_id: i64,
    pub token_addr: Option<String>,
    pub token_amount: String,
    pub tx_hash: String,
    pub block_number: i64,
    pub fee_paid: Option<String>,
    pub block_date: DateTime<Utc>,
    pub block_timestamp: i64,
    pub to_addr: String,
    pub caller_addr: String,
}

pub async fn stats_transfers(
    data: Data<Box<ServerData>>,
    info: web::Query<StatsTransferRequest>,
) -> actix_web::Result<web::Json<StatsTransferResult>> {
    let time_start = std::time::Instant::now();
    let receiver = if info.receiver.clone() == Some("all".to_string()) {
        None
    } else {
        let account = Address::from_str(
            &info
                .receiver
                .clone()
                .ok_or(actix_web::error::ErrorBadRequest("account not found"))?,
        )
        .map_err(|err| {
            actix_web::error::ErrorBadRequest(format!("account has to be valid address {err}"))
        })?;
        Some(account)
    };
    let account_str = receiver.map(|account| format!("{:#x}", account));

    let from = chrono::DateTime::from_timestamp(
        i64::from_str(
            &info
                .from
                .clone()
                .ok_or(actix_web::error::ErrorBadRequest("From not found"))?,
        )
        .map_err(|err| {
            actix_web::error::ErrorBadRequest(format!("From is not a valid timestamp {err}"))
        })?,
        0,
    )
    .ok_or(actix_web::error::ErrorBadRequest(
        "From is not a valid timestamp.",
    ))?;
    let to = chrono::DateTime::from_timestamp(
        i64::from_str(
            &info
                .to
                .clone()
                .ok_or(actix_web::error::ErrorBadRequest("To not found"))?,
        )
        .map_err(|err| {
            actix_web::error::ErrorBadRequest(format!("To is not a valid timestamp {err}"))
        })?,
        0,
    )
    .ok_or(actix_web::error::ErrorBadRequest(
        "To is not a valid timestamp.",
    ))?;

    let chain_id = i64::from_str(
        &info
            .chain
            .clone()
            .ok_or(actix_web::error::ErrorBadRequest("Chain id not found"))?,
    )
    .map_err(|err| actix_web::error::ErrorBadRequest(format!("Chain id a valid {err}")))?;

    let conn = data.db_connection.lock().await.clone();
    let transf = if let Some(receiver) = account_str.as_ref() {
        let transf =
            get_all_chain_transfers_by_receiver_ext(&conn, chain_id, from, to, receiver, None)
                .await;
        transf.map_err(|err| {
            actix_web::error::ErrorBadRequest(format!("Unknown server error: {}", err))
        })?
    } else {
        let transf = get_all_chain_transfers_ext(&conn, chain_id, from, to, None).await;
        transf.map_err(|err| {
            actix_web::error::ErrorBadRequest(format!("Unknown server error: {}", err))
        })?
    };

    let mut resp = Vec::new();
    for trans in transf.into_iter() {
        let Some(blockchain_date) = trans.blockchain_date else {
            continue;
        };

        if blockchain_date < from {
            continue;
        }
        if let Some(account_str) = account_str.as_ref() {
            if trans.receiver_addr != *account_str {
                continue;
            }
        }

        resp.push(ChainTransferRespObj {
            id: trans.id,
            from_addr: trans.from_addr,
            receiver_addr: trans.receiver_addr,
            chain_id: trans.chain_id,
            token_addr: trans.token_addr,
            token_amount: trans.token_amount,
            tx_hash: trans.tx_hash,
            block_number: trans.block_number,
            fee_paid: trans.fee_paid,
            block_date: blockchain_date,
            block_timestamp: blockchain_date.timestamp(),
            to_addr: trans.to_addr,
            caller_addr: trans.caller_addr,
        })
    }

    let time_end = time_start.elapsed().as_secs_f64();
    //serialize
    Ok(web::Json(StatsTransferResult {
        request_time: time_end,
        transfers: resp,
    }))
}

pub async fn transfers(data: Data<Box<ServerData>>, req: HttpRequest) -> impl Responder {
    let tx_id = req
        .match_info()
        .get("tx_id")
        .map(|tx_id| i64::from_str(tx_id).ok())
        .unwrap_or(None);

    //let my_data = data.shared_state.lock().await;

    let transfers = {
        let db_conn = data.db_connection.lock().await;
        if let Some(tx_id) = tx_id {
            match get_token_transfers_by_tx(&*db_conn, tx_id).await {
                Ok(allowances) => allowances,
                Err(err) => {
                    return web::Json(json!({
                        "error": err.to_string()
                    }));
                }
            }
        } else {
            match get_all_token_transfers(&db_conn, None).await {
                Ok(allowances) => allowances,
                Err(err) => {
                    return web::Json(json!({
                        "error": err.to_string()
                    }));
                }
            }
        }
    };

    /*
        let json_transfers = transfers
            .iter()
            .map(|transfer| {
                json!({
                    "id": transfer.id,
                    "chain_id": transfer.chain_id,
                    "tx_id": transfer.tx_id,
                    "from": transfer.from_addr,
                    "receiver": transfer.receiver_addr,
                    "token": transfer.token_addr,
                    "amount": transfer.token_amount,
                    "fee_paid": transfer.fee_paid,
                })
            })
            .collect::<Vec<_>>();
    */
    web::Json(json!({
        "transfers": transfers,
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountBalanceResponse {
    network_id: i64,
    account: String,
    gas_balance: String,
    token_balance: String,
    block_number: u64,
    block_date: chrono::DateTime<chrono::Utc>,
}

async fn account_balance(
    data: Data<Box<ServerData>>,
    req: HttpRequest,
) -> actix_web::Result<web::Json<AccountBalanceResponse>> {
    let account = Address::from_str(
        req.match_info()
            .get("account")
            .ok_or(actix_web::error::ErrorBadRequest("account not found"))?,
    )
    .map_err(|err| {
        actix_web::error::ErrorBadRequest(format!("account has to be valid address {err}"))
    })?;
    let network_id = i64::from_str(
        req.match_info()
            .get("chain")
            .ok_or(actix_web::error::ErrorBadRequest("chain-id not found"))?,
    )
    .map_err(|err| actix_web::error::ErrorBadRequest(format!("chain-id has to be int {err}")))?;

    let chain = data
        .payment_setup
        .chain_setup
        .get(&network_id)
        .ok_or(actix_web::error::ErrorBadRequest("No config found"))?;

    let args = GetBalanceArgs {
        address: Default::default(),
        token_address: Some(chain.glm_address),
        call_with_details: chain.wrapper_contract_address,
        block_number: None,
        chain_id: Some(chain.chain_id as u64),
    };
    let balance_result = get_balance(chain.provider.clone(), args)
        .await
        .map_err(|err| {
            actix_web::error::ErrorInternalServerError(format!("Failed to get balance {err}"))
        })?;

    Ok(web::Json(AccountBalanceResponse {
        network_id,
        account: format!("{:#x}", account),
        gas_balance: balance_result
            .gas_balance
            .map(|b| b.to_string())
            .unwrap_or("0".to_string()),
        token_balance: balance_result
            .token_balance
            .map(|b| b.to_string())
            .unwrap_or("0".to_string()),
        block_number: balance_result.block_number,
        block_date: balance_result.block_datetime,
    }))
}

pub async fn accounts(data: Data<Box<ServerData>>, _req: HttpRequest) -> impl Responder {
    //let name = req.match_info().get("name").unwrap_or("World");
    //let mut my_data = data.shared_state.lock().await;
    //my_data.inserted += 1;

    let public_addr = data
        .shared_state
        .lock()
        .unwrap()
        .accounts
        .iter()
        .map(|sk| format!("{:#x}", sk.address))
        .collect::<Vec<String>>();

    web::Json(json!({
        "publicAddr": public_addr
    }))
}

pub async fn account_payments_in(data: Data<Box<ServerData>>, req: HttpRequest) -> impl Responder {
    let account = return_on_error!(req.match_info().get("account").ok_or("No account provided"));
    let web3_account = return_on_error!(Address::from_str(account));
    let account = format!("{web3_account:#x}");

    let transfers_in = {
        let db_conn = data.db_connection.lock().await;
        return_on_error!(get_account_transfers_in(&db_conn, &account, None).await)
    };
    /*let chain_transfers = {
        let db_conn = data.db_connection.lock().await;
        return_on_error!(get_account_chain_transfers(&db_conn, &account).await)
    };*/

    web::Json(json!({
        "transfersIn": transfers_in,
     //   "chainTransfers": chain_transfers,
    }))
}

pub async fn account_details(data: Data<Box<ServerData>>, req: HttpRequest) -> impl Responder {
    let account = return_on_error!(req.match_info().get("account").ok_or("No account provided"));

    let web3_account = return_on_error!(Address::from_str(account));

    let account = format!("{web3_account:#x}");

    let is_sender = if let Some(addr) = data
        .shared_state
        .lock()
        .unwrap()
        .accounts
        .iter()
        .map(|acc| format!("{:#x}", acc.address))
        .find(|addr| addr == &account)
    {
        log::debug!("Found account: {}", addr);
        true
    } else {
        false
    };
    let allowances = {
        let db_conn = data.db_connection.lock().await;
        return_on_error!(get_allowances_by_owner(&db_conn, &account).await)
    };

    let mut queued_transfer_count = 0;
    let mut processed_transfer_count = 0;
    let mut done_transfer_count = 0;

    if is_sender {
        queued_transfer_count = {
            let db_conn = data.db_connection.lock().await;
            return_on_error!(
                get_transfer_count(&db_conn, Some(TRANSFER_FILTER_QUEUED), Some(&account), None)
                    .await
            )
        };
        processed_transfer_count = {
            let db_conn = data.db_connection.lock().await;
            return_on_error!(
                get_transfer_count(
                    &db_conn,
                    Some(TRANSFER_FILTER_PROCESSING),
                    Some(&account),
                    None
                )
                .await
            )
        };
        done_transfer_count = {
            let db_conn = data.db_connection.lock().await;
            return_on_error!(
                get_transfer_count(&db_conn, Some(TRANSFER_FILTER_DONE), Some(&account), None)
                    .await
            )
        };
    }
    let received_transfer_count = {
        let db_conn = data.db_connection.lock().await;

        return_on_error!(
            get_transfer_count(&db_conn, Some(TRANSFER_FILTER_ALL), None, Some(&account)).await
        )
    };

    web::Json(json!({
        "account": account,
        "allowances": allowances,
        "transfersQueued": queued_transfer_count,
        "transfersProcessing": processed_transfer_count,
        "transfersDone": done_transfer_count,
        "receivedTransfers": received_transfer_count,
    }))
}

pub async fn redirect_to_slash(req: HttpRequest) -> impl Responder {
    let mut response = HttpResponse::Ok();
    let target = match HeaderValue::from_str(&(req.uri().to_string() + "/")) {
        Ok(target) => target,
        Err(_err) => {
            return HttpResponse::InternalServerError().body("Failed to create redirect target");
        }
    };

    response
        .status(StatusCode::PERMANENT_REDIRECT)
        .append_header((header::LOCATION, target))
        .finish()
}

pub async fn metrics(_req: HttpRequest) -> impl Responder {
    export_metrics_to_prometheus().unwrap_or_else(|err| {
        log::error!("Failed to export metrics: {}", err);
        format!("Failed to export metrics: {}", err)
    })
}

pub async fn greet(_req: HttpRequest) -> impl Responder {
    const VERSION: &str = env!("CARGO_PKG_VERSION");
    web::Json(json!({
        "name": "erc20_payment_lib",
        "version": VERSION,
    }))
}

pub async fn faucet(data: Data<Box<ServerData>>, req: HttpRequest) -> impl Responder {
    let target_addr = req.match_info().get("addr").unwrap_or("");
    let chain_id = req.match_info().get("chain").unwrap_or("");
    if !target_addr.is_empty() {
        let receiver_addr = return_on_error!(web3::types::Address::from_str(target_addr));

        let chain_id = return_on_error!(i64::from_str(chain_id));

        let chain: &ChainSetup = return_on_error!(data
            .payment_setup
            .chain_setup
            .get(&(chain_id))
            .ok_or("No config for given chain id"));
        let faucet_event_idx = format!("{receiver_addr:#x}_{chain_id}");

        {
            let mut shared_state = data.shared_state.lock().unwrap();
            let faucet_data = match shared_state.faucet {
                Some(ref mut faucet_data) => faucet_data,
                None => {
                    shared_state.faucet = Some(FaucetData {
                        faucet_events: BTreeMap::new(),
                        last_cleanup: chrono::Utc::now(),
                    });
                    shared_state
                        .faucet
                        .as_mut()
                        .expect("Faucet data should be set here")
                }
            };

            const MIN_SECONDS: i64 = 120;
            if let Some(el) = faucet_data.faucet_events.get(&faucet_event_idx) {
                let ago = (chrono::Utc::now().time() - el.time()).num_seconds();
                if ago < MIN_SECONDS {
                    return web::Json(json!({
                        "error": format!("Already sent to this address {ago} seconds ago. Try again after {MIN_SECONDS} seconds")
                    }));
                } else {
                    faucet_data
                        .faucet_events
                        .insert(faucet_event_idx, chrono::Utc::now());
                }
            } else {
                faucet_data
                    .faucet_events
                    .insert(faucet_event_idx, chrono::Utc::now());
            }

            //faucet data cleanup
            const FAUCET_CLEANUP_AFTER: i64 = 120;
            let curr_time = chrono::Utc::now();
            if (curr_time.time() - faucet_data.last_cleanup.time()).num_seconds()
                > FAUCET_CLEANUP_AFTER
            {
                faucet_data.last_cleanup = curr_time;
                faucet_data
                    .faucet_events
                    .retain(|_, v| (curr_time.time() - v.time()).num_seconds() < MIN_SECONDS);
            }
        }

        let glm_address = chain.glm_address;

        let from = data
            .shared_state
            .lock()
            .unwrap()
            .accounts
            .first()
            .unwrap()
            .address;

        let faucet_eth_amount = return_on_error!(chain
            .faucet_eth_amount
            .ok_or("Faucet amount not set on chain"));
        let faucet_glm_amount = return_on_error!(chain
            .faucet_glm_amount
            .ok_or("Faucet GLM amount not set on chain"));

        let token_transfer_eth = {
            let tt = create_token_transfer(
                from,
                receiver_addr,
                chain_id,
                Some(&uuid::Uuid::new_v4().to_string()),
                None,
                faucet_eth_amount,
                None,
            );
            let db_conn = data.db_connection.lock().await;
            return_on_error!(insert_token_transfer(&*db_conn, &tt).await)
        };
        let token_transfer_glm = {
            let tt = create_token_transfer(
                from,
                receiver_addr,
                chain_id,
                Some(&uuid::Uuid::new_v4().to_string()),
                Some(glm_address),
                faucet_glm_amount,
                None,
            );
            let db_conn = data.db_connection.lock().await;
            return_on_error!(insert_token_transfer(&*db_conn, &tt).await)
        };

        return web::Json(json!({
        "transfer_gas_id": token_transfer_eth.id,
        "transfer_gas_payment_id": token_transfer_eth.payment_id,
        "transfer_glm_id": token_transfer_glm.id,
        "transfer_glm_payment_id": token_transfer_glm.payment_id,
                }));
    }

    web::Json(json!({
        "status": "faucet enabled"
    }))
}

#[derive(Debug, Serialize)]
struct AttestationItemInfo {
    name: String,
    #[serde(rename = "type")]
    typ: String,
    value: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttestationCheckResult {
    chain_id: u64,
    chain: String,
    attestation: Attestation,
    schema: AttestationSchema,
    params: Vec<AttestationItemInfo>,
}

fn ethabi_token_to_json(token: &ethabi::Token) -> serde_json::Value {
    match token {
        ethabi::Token::Address(addr) => serde_json::Value::String(format!("{:#x}", addr)),
        ethabi::Token::FixedBytes(bytes) => {
            serde_json::Value::String(format!("0x{}", hex::encode(bytes)))
        }
        ethabi::Token::Int(int) => {
            if int <= &U256::from(2147483647) {
                serde_json::Value::Number(serde_json::Number::from(int.as_u32()))
            } else {
                serde_json::Value::String(format!("{}", int))
            }
        }
        ethabi::Token::Uint(uint) => {
            if uint <= &U256::from(2147483647) {
                serde_json::Value::Number(serde_json::Number::from(uint.as_u32()))
            } else {
                serde_json::Value::String(format!("{}", uint))
            }
        }
        ethabi::Token::Bool(b) => serde_json::Value::Bool(*b),
        ethabi::Token::String(s) => serde_json::Value::String(s.clone()),
        ethabi::Token::Bytes(bytes) => {
            serde_json::Value::String(format!("0x{}", hex::encode(bytes)))
        }
        ethabi::Token::Array(vec) | ethabi::Token::FixedArray(vec) | ethabi::Token::Tuple(vec) => {
            serde_json::Value::Array(
                vec.iter()
                    .map(ethabi_token_to_json)
                    .collect::<Vec<serde_json::Value>>(),
            )
        }
    }
}

pub async fn check_attestation(
    data: Data<Box<ServerData>>,
    req: HttpRequest,
) -> actix_web::Result<web::Json<AttestationCheckResult>> {
    let attestation_uid = req.match_info().get("uid").unwrap_or("");
    let chain_name = req.match_info().get("chain").unwrap_or("");
    let chain: &ChainSetup = data
        .payment_setup
        .chain_setup
        .iter()
        .find(|(_, chain)| chain.network == chain_name)
        .ok_or(actix_web::error::ErrorBadRequest(format!(
            "No config found for network {}",
            chain_name
        )))?
        .1;

    let web3 = data
        .payment_setup
        .get_provider(chain.chain_id)
        .map_err(|e| ErrorBadRequest(format!("Failed to get provider: {}", e)))?;

    let decoded_bytes = match hex::decode(attestation_uid.replace("0x", "")) {
        Ok(bytes) => bytes,
        Err(e) => {
            return Err(ErrorBadRequest(format!(
                "Failed to decode attestation id: {}",
                e
            )));
        }
    };

    let contract = chain
        .eas_contract_settings
        .clone()
        .ok_or(ErrorBadRequest(format!(
            "No contract settings found for chain {}",
            chain_name
        )))?;

    let schema_contract = chain
        .eas_schema_registry_settings
        .clone()
        .ok_or(ErrorBadRequest(format!(
            "No schema contract settings found for chain {}",
            chain_name
        )))?;

    let uid = ethabi::Bytes::from(decoded_bytes);

    let uid = if uid.len() != 32 {
        return Err(ErrorBadRequest(format!(
            "Invalid attestation id length: {}, expected 32",
            uid.len()
        )));
    } else {
        H256::from_slice(uid.as_slice())
    };
    log::info!("Querying attestation contract: {:#x}", contract.address);

    let attestation = match get_attestation_details(web3.clone(), uid, contract.address).await {
        Ok(Some(attestation)) => attestation,
        Ok(None) => {
            return Err(ErrorBadRequest(format!(
                "Attestation with uid: {:#x} not found on chain {}",
                uid, chain_name
            )));
        }
        Err(e) => {
            log::error!("Failed to get attestation details: {}", e);
            return Err(ErrorBadRequest(format!(
                "Failed to get attestation details: {}",
                e
            )));
        }
    };

    let attestation_schema =
        match get_schema_details(web3, attestation.schema, schema_contract.address).await {
            Ok(attestation_schema) => attestation_schema,
            Err(e) => {
                log::error!("Failed to get attestation details: {}", e);
                return Err(ErrorBadRequest(format!(
                    "Failed to get attestation details: {}",
                    e
                )));
            }
        };

    log::info!("Querying schema contract: {:#x}", schema_contract.address);

    println!(
        "attestation: {}",
        serde_json::to_string_pretty(&attestation).map_err(|e| ErrorBadRequest(format!(
            "Failed to serialize attestation details: {}",
            e
        )))?
    );

    println!(
        "schema: {}",
        serde_json::to_string_pretty(&attestation_schema).map_err(|e| ErrorBadRequest(format!(
            "Failed to serialize attestation details: {}",
            e
        )))?
    );

    let items = attestation_schema.schema.split(',').collect::<Vec<&str>>();
    log::debug!("There are {} items in the schema", items.len());
    let mut param_types = Vec::new();
    let mut param_names = Vec::new();

    for item in items {
        let items2 = item.trim().split(' ').collect::<Vec<&str>>();
        if items2.len() != 2 {
            log::error!("Invalid item in schema: {}", item);
            return Err(ErrorBadRequest(format!("Invalid item in schema: {}", item)));
        }
        let item_type = items2[0].trim();
        let item_name = items2[1].trim();

        log::debug!("Item name: {}, Item type: {}", item_name, item_type);
        let param_type = ethabi::param_type::Reader::read(item_type)
            .map_err(|e| ErrorBadRequest(format!("Failed to read param type: {}", e)))?;
        param_types.push(param_type);
        param_names.push(item_name);
    }

    let decoded_tokens = ethabi::decode(&param_types, &attestation.data.0)
        .map_err(|e| ErrorBadRequest(format!("Failed to decode attestation data: {}", e)))?;

    let mut decoded_items = Vec::new();
    for ((token, token_name), token_type) in decoded_tokens
        .iter()
        .zip(param_names.iter())
        .zip(param_types.iter())
    {
        println!("Token {}: {}", token_name, token);
        decoded_items.push(AttestationItemInfo {
            name: token_name.to_string(),
            typ: token_type.to_string(),
            value: ethabi_token_to_json(token),
        });
    }

    Ok(web::Json(AttestationCheckResult {
        chain_id: chain.chain_id as u64,
        chain: chain_name.to_string(),
        attestation,
        schema: attestation_schema,
        params: decoded_items,
    }))
}

pub fn runtime_web_scope(
    scope: Scope,
    server_data: Data<Box<ServerData>>,
    enable_faucet: bool,
    enable_transfers: bool,
    debug: bool,
    frontend: bool,
) -> Scope {
    let api_scope = Scope::new("/api");
    let mut api_scope = api_scope
        .app_data(server_data)
        .route(
            "/attestation/{chain}/{uid}",
            web::get().to(check_attestation),
        )
        .route("/allowances", web::get().to(allowances))
        .route("/balance/{account}/{chain}", web::get().to(account_balance))
        .route("/rpc_pool", web::get().to(rpc_pool))
        .route("/rpc_pool/metrics", web::get().to(rpc_pool_metrics))
        .route("/config", web::get().to(config_endpoint))
        .route("/stats/transfers", web::get().to(stats_transfers))
        .route("/transactions", web::get().to(transactions))
        .route("/transactions/count", web::get().to(transactions_count))
        .route("/transactions/next", web::get().to(transactions_next))
        .route(
            "/transactions/feed/{prev}/{next}",
            web::get().to(transactions_feed),
        )
        .route(
            "/transactions/next/{count}",
            web::get().to(transactions_next),
        )
        .route("/transactions/current", web::get().to(transactions_current))
        .route(
            "/transactions/last",
            web::get().to(transactions_last_processed),
        )
        .route(
            "/transactions/last/{count}",
            web::get().to(transactions_last_processed),
        )
        .route("/tx/skip/{tx_id}", web::post().to(skip_pending_operation))
        .route("/tx/{tx_id}", web::get().to(tx_details))
        .route("/transfers", web::get().to(transfers))
        .route("/transfers/{tx_id}", web::get().to(transfers))
        .route("/accounts", web::get().to(accounts))
        .route("/account/{account}", web::get().to(account_details))
        .route("/account/{account}/in", web::get().to(account_payments_in))
        .route("/metrics", web::get().to(metrics))
        .route("/", web::get().to(greet))
        .route(
            "/event_stream",
            web::get().to(event_stream_websocket_endpoint),
        )
        .route("/version", web::get().to(greet));

    if enable_transfers {
        api_scope = api_scope.route("/transfers/new", web::post().to(new_transfer))
    }
    if enable_faucet {
        log::info!("Faucet endpoints enabled");
        api_scope = api_scope.route("/faucet", web::get().to(faucet));
        api_scope = api_scope.route("/faucet/{chain}/{addr}", web::get().to(faucet));
    }
    if debug {
        log::info!("Debug endpoints enabled");
        api_scope = api_scope.route("/debug", web::get().to(debug_endpoint));
    }

    // Add version endpoint to /api, /api/ and /api/version
    let scope = scope.route("/api", web::get().to(greet));
    let mut scope = scope.service(api_scope);

    if frontend {
        log::info!("Frontend endpoint enabled");
        //This has to be on end, otherwise it catches requests to backend
        let static_files = actix_files::Files::new("/frontend", "./frontend/dist")
            .index_file("index.html")
            .default_handler(|req: ServiceRequest| {
                let (http_req, _payload) = req.into_parts();

                async {
                    let response = NamedFile::open("./frontend/dist/index.html")
                        .unwrap()
                        .into_response(&http_req);
                    Ok(ServiceResponse::new(http_req, response))
                }
            });

        scope = scope.route("/frontend", web::get().to(redirect_to_slash));
        scope = scope.service(static_files);
    }
    scope
}
