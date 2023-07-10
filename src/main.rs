mod options;
use crate::options::{PaymentCommands, PaymentOptions};
use actix_web::Scope;
use actix_web::{web, App, HttpServer};
use csv::{ReaderBuilder, WriterBuilder};
use erc20_payment_lib::config::AdditionalOptions;
use erc20_payment_lib::db::create_sqlite_connection;
use erc20_payment_lib::db::model::TokenTransferDao;
use erc20_payment_lib::db::ops::{do_db_operation, get_transfer_count, insert_token_transfer};
use erc20_payment_lib::misc::{
    create_test_amount_pool, generate_transaction_batch, ordered_address_pool,
};
use erc20_payment_lib::server::*;

use erc20_payment_lib::{
    config, err_create, err_custom_create, err_from,
    error::{CustomError, ErrorBag, PaymentError},
    misc::{display_private_keys, load_private_keys},
    runtime::start_payment_engine,
};
use futures::{StreamExt, TryStreamExt};
use std::env;

use std::sync::{atomic, Arc};
use std::time::{Duration, Instant};
use structopt::StructOpt;
use tokio::sync::Mutex;

async fn main_internal() -> Result<(), PaymentError> {
    dotenv::dotenv().ok();
    env::set_var(
        "RUST_LOG",
        env::var("RUST_LOG").unwrap_or("info,sqlx::query=warn,web3=warn".to_string()),
    );

    env_logger::init();
    let cli: PaymentOptions = PaymentOptions::from_args();

    let (private_keys, public_addrs) = load_private_keys(
        &env::var("ETH_PRIVATE_KEYS").expect("Specify ETH_PRIVATE_KEYS env variable"),
    )?;
    display_private_keys(&private_keys);

    let config = config::Config::load("config-payments.toml")?;

    match cli.commands {
        PaymentCommands::Run { run_options } => {
            if run_options.http && !run_options.keep_running {
                return Err(err_custom_create!("http mode requires keep-running option"));
            }

            let add_opt = AdditionalOptions {
                keep_running: run_options.keep_running,
                generate_tx_only: run_options.generate_tx_only,
                skip_multi_contract_check: run_options.skip_multi_contract_check,
            };
            let db_filename =
                env::var("DB_SQLITE_FILENAME").expect("Specify DB_SQLITE_FILENAME env variable");
            log::info!("connecting to sqlite file db: {}", db_filename);
            let conn = create_sqlite_connection(Some(&db_filename), true).await?;

            let sp = start_payment_engine(
                &private_keys,
                &db_filename,
                config,
                Some(conn.clone()),
                Some(add_opt),
            )
            .await?;

            let server_data = web::Data::new(Box::new(ServerData {
                shared_state: sp.shared_state.clone(),
                db_connection: Arc::new(Mutex::new(conn)),
                payment_setup: sp.setup.clone(),
            }));

            if run_options.http {
                let server = HttpServer::new(move || {
                    let cors = actix_cors::Cors::default()
                        .allow_any_origin()
                        .allow_any_method()
                        .allow_any_header()
                        .max_age(3600);

                    let scope = runtime_web_scope(
                        Scope::new("erc20"),
                        server_data.clone(),
                        run_options.faucet,
                        run_options.debug,
                        run_options.frontend,
                    );

                    App::new().wrap(cors).service(scope)
                })
                .workers(run_options.http_threads as usize)
                .bind((run_options.http_addr.as_str(), run_options.http_port))
                .expect("Cannot run server")
                .run();

                log::info!(
                    "http server starting on {}:{}",
                    run_options.http_addr,
                    run_options.http_port
                );

                server.await.unwrap();
            } else {
                sp.runtime_handle.await.unwrap();
            }
        }
        PaymentCommands::GenerateTestPayments { generate_options } => {
            let chain_cfg =
                config
                    .chain
                    .get(&generate_options.chain_name)
                    .ok_or(err_custom_create!(
                        "Chain {} not found in config file",
                        generate_options.chain_name
                    ))?;
            let addr_pool = ordered_address_pool(generate_options.address_pool_size, false)?;
            let amount_pool = create_test_amount_pool(generate_options.amounts_pool_size)?;

            let writer = if let Some(file) = generate_options.file {
                Some(
                    WriterBuilder::new()
                        .delimiter(b'|')
                        .from_writer(std::fs::File::create(file).map_err(err_from!())?),
                )
            } else {
                None
            };
            let writer = Arc::new(Mutex::new(writer));

            let conn = if generate_options.append_to_db {
                let db_filename = env::var("DB_SQLITE_FILENAME")
                    .expect("Specify DB_SQLITE_FILENAME env variable");
                log::info!("connecting to sqlite file db: {}", db_filename);
                let conn = create_sqlite_connection(Some(&db_filename), true).await?;
                Some(conn)
            } else {
                None
            };

            let started = Instant::now();
            let mut stream_delay = Arc::new(Mutex::new(0.0));

            match generate_transaction_batch(
                chain_cfg.chain_id,
                &public_addrs,
                Some(chain_cfg.token.clone().unwrap().address),
                &addr_pool,
                &amount_pool,
            )?
            .then(|res| {
                let stream_delay = stream_delay.clone();
                async move {
                    if let (Ok((transfer_no, _)), Some(interval)) =
                        (&res, generate_options.interval)
                    {
                        const MAX_SLIPPAGE_INTERVALS: f64 = 10.0;
                        let target_time_point =
                            *transfer_no as f64 * interval + *stream_delay.lock().await;
                        let elapsed = started.elapsed();
                        let delta = target_time_point - elapsed.as_secs_f64();
                        let wait_time_seconds = if delta > 0.0 {
                            delta
                        } else {
                            //try to catch up, but not too much (up to twice as fast)
                            interval * 0.5
                        };
                        if delta < -MAX_SLIPPAGE_INTERVALS * interval {
                            *stream_delay.lock().await -= delta;
                            log::warn!(
                                "Stream is falling behind, current delay {}s",
                                *stream_delay.lock().await
                            );
                        }
                        tokio::time::sleep(Duration::from_secs_f64(wait_time_seconds)).await;
                    }
                    res
                }
            })
            .take(generate_options.generate_count)
            .try_for_each(move |(transfer_no, token_transfer)| {
                let writer = writer.clone();
                let conn = conn.clone();

                async move {
                    if let Some(limit_time) = generate_options.limit_time {
                        // check how much time has passed since start
                        let elapsed = started.elapsed();
                        if elapsed.as_secs_f64() > limit_time {
                            return Err(err_create!(elapsed));
                        }
                    };
                    let mut writer = writer.lock().await;
                    let res = if let Some(writer) = writer.as_mut() {
                        writer.serialize(&token_transfer).map_err(|err| {
                            log::error!("error writing csv record: {}", err);
                            err_custom_create!("error writing csv record: {err}")
                        })
                    } else {
                        log::info!(
                            "Generated tx no {} to: {}",
                            transfer_no,
                            token_transfer.receiver_addr
                        );
                        Ok(())
                    };
                    if let Some(conn) = conn {
                        let _token_transfer =
                            do_db_operation(|| insert_token_transfer(&conn, &token_transfer))
                                .await
                                .map_err(|err| {
                                    err_custom_create!(
                                        "Error writing record to db no: {transfer_no}, err: {err}"
                                    )
                                })?;
                    }
                    res
                }
            })
            .await
            {
                Ok(_) => {
                    log::info!("All transactions generated successfully");
                }
                Err(err) => match err.inner {
                    ErrorBag::TimeLimitReached(d) => {
                        log::info!("Time limit reached: {} seconds, exiting", d.as_secs_f64());
                    }
                    _ => return Err(err),
                },
            };
        }
        PaymentCommands::PaymentStatistics {
            payment_statistics_options: _,
        } => {
            println!("payment statistics");
            let db_filename =
                env::var("DB_SQLITE_FILENAME").expect("Specify DB_SQLITE_FILENAME env variable");
            log::info!("connecting to sqlite file db: {}", db_filename);
            let conn = create_sqlite_connection(Some(&db_filename), true).await?;
            println!(
                "Token transfer count: {}",
                get_transfer_count(&conn, None, None, None).await.unwrap()
            );
        }
        PaymentCommands::ImportPayments { import_options } => {
            log::info!("importing payments from file: {}", import_options.file);
            //import_options.file;
            let mut rdr = ReaderBuilder::new()
                .delimiter(import_options.separator as u8)
                .from_reader(std::fs::File::open(&import_options.file).map_err(err_from!())?);

            let deserialize = rdr.deserialize::<TokenTransferDao>();
            let db_filename =
                env::var("DB_SQLITE_FILENAME").expect("Specify DB_SQLITE_FILENAME env variable");
            log::info!("connecting to sqlite file db: {}", db_filename);
            let conn = create_sqlite_connection(Some(&db_filename), true).await?;

            let mut token_transfer_list = vec![];
            for (line_no, result) in deserialize.enumerate() {
                match result {
                    Ok(token_transfer) => {
                        let chain_cfg = config
                            .chain
                            .values()
                            .find(|el| el.chain_id == token_transfer.chain_id)
                            .ok_or(err_custom_create!(
                                "Chain id {} not found in config file",
                                token_transfer.chain_id
                            ))?;

                        if let (Some(token_chain_cfg), Some(token_addr)) =
                            (&chain_cfg.token, &token_transfer.token_addr)
                        {
                            if format!("{:#x}", token_chain_cfg.address)
                                != token_addr.to_lowercase()
                            {
                                return Err(err_custom_create!(
                                    "Token address in line {} is different from default token address {} != {:#x}",
                                    line_no,
                                    token_addr.to_lowercase(),
                                    token_chain_cfg.address
                                ));
                            }
                        }

                        token_transfer_list.push(token_transfer);
                    }
                    Err(e) => {
                        log::error!("Error reading data from CSV {:?}", e);
                        break;
                    }
                }
            }
            log::info!(
                "Found {} transfers in {}, inserting to db...",
                token_transfer_list.len(),
                import_options.file
            );
            for token_transfer in token_transfer_list {
                insert_token_transfer(&conn, &token_transfer)
                    .await
                    .map_err(err_from!())?;
            }
        }
        PaymentCommands::DecryptKeyStore { decrypt_options } => {
            let pkey = eth_keystore::decrypt_key(
                decrypt_options.file,
                decrypt_options.password.unwrap_or_default(),
            )
            .unwrap();
            println!("Private key: {}", hex::encode(pkey));
        }
    }

    Ok(())
}

#[actix_web::main]
async fn main() -> Result<(), PaymentError> {
    match main_internal().await {
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("Error: {e}");
            Err(e)
        }
    }
}
