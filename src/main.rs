mod actions;
mod options;
mod stats;

use crate::options::{AttestationCommands, DepositCommands, PaymentCommands, PaymentOptions};
use actix_web::Scope;
use actix_web::{web, App, HttpServer};
use csv::ReaderBuilder;
use erc20_payment_lib::config::{AdditionalOptions, RpcSettings};
use erc20_payment_lib::signer::PrivateKeySigner;
use erc20_payment_lib_common::create_sqlite_connection;
use erc20_payment_lib_common::error::*;
use erc20_payment_lib_common::ops::{
    get_next_transactions_to_process, insert_token_transfer,
    insert_token_transfer_with_deposit_check, update_token_transfer,
};
use erc20_payment_lib_common::*;

use crate::actions::scan_chain::scan_blockchain_local;
use erc20_payment_lib::{
    config,
    misc::{display_private_keys, load_private_keys},
    runtime::PaymentRuntime,
};
use std::env;
use std::str::FromStr;

use crate::actions::attestation::check::check_attestation_local;
use crate::actions::check_address_name;
use crate::actions::check_rpc::check_rpc_local;
use crate::actions::deposit::close::close_deposit_local;
use crate::actions::deposit::create::make_deposit_local;
use crate::actions::deposit::details::deposit_details_local;
use crate::actions::deposit::terminate::terminate_deposit_local;
use crate::stats::{export_stats, run_stats};
use erc20_payment_lib::faucet_client::faucet_donate;
use erc20_payment_lib::misc::gen_private_keys;
use erc20_payment_lib::runtime::{
    distribute_gas, get_token_balance, mint_golem_token, remove_last_unsent_transactions,
    remove_transaction_force, PaymentRuntimeArgs,
};
use erc20_payment_lib::server::web::{runtime_web_scope, ServerData};
use erc20_payment_lib::setup::PaymentSetup;
use erc20_payment_lib_common::init_metrics;
use erc20_payment_lib_common::model::{DepositId, TokenTransferDbObj};
use erc20_payment_lib_common::utils::{DecimalConvExt, StringConvExt};
use erc20_payment_lib_extra::{account_balance, generate_test_payments};
use rust_decimal::Decimal;
use std::sync::Arc;
use structopt::StructOpt;
use tokio::sync::{broadcast, Mutex};
use web3::types::U256;
use erc20_payment_lib::eth::GetBalanceArgs;

async fn main_internal() -> Result<(), PaymentError> {
    dotenv::dotenv().ok();
    env::set_var(
        "RUST_LOG",
        env::var("RUST_LOG").unwrap_or("info,sqlx::query=info,web3=warn".to_string()),
    );

    env_logger::init();
    init_metrics();
    let cli: PaymentOptions = PaymentOptions::from_args();

    let mut private_key_load_needed = true;
    let mut db_connection_needed = true;

    match cli.commands {
        PaymentCommands::Run { .. } => {}
        PaymentCommands::Generate { .. } => {}
        PaymentCommands::GenerateKey { .. } => {
            private_key_load_needed = false;
            db_connection_needed = false;
        }
        PaymentCommands::CheckRpc { .. } => {}
        PaymentCommands::GetDevEth { .. } => {}
        PaymentCommands::MintTestTokens { .. } => {}
        PaymentCommands::Deposit { .. } => {}
        PaymentCommands::Transfer { .. } => {}
        PaymentCommands::Distribute { .. } => {}
        PaymentCommands::Balance { .. } => {}
        PaymentCommands::ImportPayments { .. } => {}
        PaymentCommands::ScanBlockchain { .. } => {}
        PaymentCommands::PaymentStats { .. } => {}
        PaymentCommands::ExportHistory { .. } => {}
        PaymentCommands::DecryptKeyStore { .. } => {}
        PaymentCommands::Cleanup { .. } => {}
        PaymentCommands::ShowConfig { .. } => {}
        PaymentCommands::Attestation { .. } => {
            private_key_load_needed = false;
        }
    }

    let (private_keys, public_addrs) = if private_key_load_needed {
        let (private_keys, public_addrs) =
            load_private_keys(&env::var("ETH_PRIVATE_KEYS").unwrap_or("".to_string()))?;
        display_private_keys(&private_keys);
        (private_keys, public_addrs)
    } else {
        (vec![], vec![])
    };
    let signer = PrivateKeySigner::new(private_keys.clone());

    let mut config = match config::Config::load("config-payments.toml").await {
        Ok(c) => c,
        Err(err) => match err.inner {
            ErrorBag::IoError(_) => {
                log::info!("No local config found, using default config");
                config::Config::default_config()
            }
            _ => return Err(err),
        },
    };

    let rpc_endpoints_from_env = [
        ("POLYGON_GETH_ADDR", "polygon"),
        ("GOERLI_GETH_ADDR", "goerli"),
        ("MUMBAI_GETH_ADDR", "mumbai"),
        ("DEV_GETH_ADDR", "dev"),
    ];

    for f in rpc_endpoints_from_env {
        if let Ok(polygon_geth_addr) = env::var(f.0) {
            let strs = polygon_geth_addr
                .split(',')
                .map(|s| s.to_string())
                .collect::<Vec<String>>();
            log::info!("Overriding default rpc endpoints for {}", f.0,);

            let rpcs = strs
                .iter()
                .map(|s| RpcSettings {
                    names: Some("ENV_RPC".to_string()),
                    endpoints: Some(s.clone()),
                    skip_validation: None,
                    verify_interval_secs: None,
                    min_interval_ms: None,
                    max_timeout_ms: None,
                    allowed_head_behind_secs: None,
                    backup_level: None,
                    max_consecutive_errors: None,
                    dns_source: None,
                    json_source: None,
                })
                .collect();
            config.change_rpc_endpoints(f.1, rpcs).await?;
        }
    }

    let max_fee_from_env = [
        ("POLYGON_MAX_BASE_FEE", "polygon"),
        ("GOERLI_MAX_BASE_FEE", "goerli"),
        ("MUMBAI_MAX_BASE_FEE", "mumbai"),
        ("DEV_MAX_BASE_FEE", "dev"),
    ];

    for f in max_fee_from_env {
        if let Ok(base_fee_from_env) = env::var(f.0) {
            let fee_per_gas = Decimal::from_str(&base_fee_from_env)
                .map_err(|_| err_custom_create!("Failed to parse max base fee"))?;
            log::info!(
                "Overriding default max base fee for {} with {}",
                f.0,
                fee_per_gas
            );
            config.change_max_fee(f.1, fee_per_gas).await?;
        }
    }

    let db_filename = cli.sqlite_db_file;
    let conn = if db_connection_needed {
        if cli.sqlite_read_only {
            log::info!(
                "Connecting read only to db: {} (journal mode: {})",
                db_filename.display(),
                cli.sqlite_journal
            );
        } else {
            log::info!(
                "Connecting read/write connection to db: {} (journal mode: {})",
                db_filename.display(),
                cli.sqlite_journal
            );
        }
        env::set_var("ERC20_LIB_SQLITE_JOURNAL_MODE", cli.sqlite_journal);
        let conn = create_sqlite_connection(
            Some(&db_filename),
            None,
            cli.sqlite_read_only,
            !cli.skip_migrations,
        )
        .await?;
        Some(conn)
    } else {
        None
    };

    match cli.commands {
        PaymentCommands::Run { run_options } => {
            if run_options.http && !run_options.keep_running {
                return Err(err_custom_create!("http mode requires keep-running option"));
            }
            if cli.sqlite_read_only {
                log::warn!("Running in read-only mode, no db writes will be possible");
            }

            let add_opt = AdditionalOptions {
                keep_running: run_options.keep_running,
                skip_service_loop: run_options.skip_service_loop,
                generate_tx_only: run_options.generate_tx_only,
                skip_multi_contract_check: run_options.skip_multi_contract_check,
                ..Default::default()
            };

            let extra_testing_options = run_options.balance_check_loop.map(|balance_check_loop| {
                erc20_payment_lib::setup::ExtraOptionsForTesting {
                    balance_check_loop: Some(balance_check_loop),
                    erc20_lib_test_replacement_timeout: None,
                }
            });

            let (broadcast_sender, broadcast_receiver) = broadcast::channel(10);
            let sp = PaymentRuntime::new(
                PaymentRuntimeArgs {
                    secret_keys: private_keys,
                    db_filename,
                    config,
                    conn: Some(conn.clone().unwrap()),
                    options: Some(add_opt),
                    mspc_sender: None,
                    broadcast_sender: Some(broadcast_sender),
                    extra_testing: extra_testing_options,
                },
                Arc::new(Box::new(signer)),
            )
            .await?;

            if run_options.http {
                let server_data = web::Data::new(Box::new(ServerData {
                    shared_state: sp.shared_state.clone(),
                    db_connection: Arc::new(Mutex::new(conn.clone().unwrap())),
                    payment_setup: sp.setup.clone(),
                    payment_runtime: sp,
                }));

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
                        run_options.transfers,
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
                sp.join_tasks().await.unwrap();
            }
            drop(broadcast_receiver);
        }
        PaymentCommands::CheckRpc {
            check_web3_rpc_options,
        } => {
            check_rpc_local(check_web3_rpc_options, config).await?;
        }
        PaymentCommands::Distribute { distribute_options } => {
            let public_addr = if let Some(address) = distribute_options.address {
                address
            } else if let Some(account_no) = distribute_options.account_no {
                *public_addrs
                    .get(account_no)
                    .expect("No public adss found with specified account_no")
            } else {
                *public_addrs.first().expect("No public adss found")
            };
            let chain_cfg =
                config
                    .chain
                    .get(&distribute_options.chain_name)
                    .ok_or(err_custom_create!(
                        "Chain {} not found in config file",
                        distribute_options.chain_name
                    ))?;

            let payment_setup = PaymentSetup::new_empty(&config)?;
            let web3 = payment_setup.get_provider(chain_cfg.chain_id)?;

            let mut recipients = Vec::with_capacity(distribute_options.recipients.len());

            for recipient in distribute_options.recipients.split(';') {
                let recipient = recipient.trim();
                recipients.push(check_address_name(recipient).map_err(|e| {
                    err_custom_create!("Invalid recipient address {}, {}", recipient, e)
                })?);
            }

            let amounts = distribute_options
                .amounts
                .split(';')
                .map(|s| {
                    let s = s.trim();
                    Decimal::from_str(s)
                        .map_err(|e| err_custom_create!("Invalid amount {}, {}", s, e))
                })
                .collect::<Result<Vec<Decimal>, PaymentError>>()?;

            if amounts.len() != recipients.len() {
                return Err(err_custom_create!(
                    "Number of recipients and amounts must be the same"
                ));
            }

            distribute_gas(
                web3,
                &conn.clone().unwrap(),
                chain_cfg.chain_id as u64,
                public_addr,
                chain_cfg.distributor_contract.clone().map(|c| c.address),
                false,
                &recipients,
                &amounts,
            )
            .await?;
        }
        PaymentCommands::GetDevEth {
            get_dev_eth_options,
        } => {
            log::info!("Getting funds from faucet...");
            let public_addr = if let Some(address) = get_dev_eth_options.address {
                address
            } else if let Some(account_no) = get_dev_eth_options.account_no {
                *public_addrs
                    .get(account_no)
                    .expect("No public adss found with specified account_no")
            } else {
                *public_addrs.first().expect("No public adss found")
            };
            let chain_cfg =
                config
                    .chain
                    .get(&get_dev_eth_options.chain_name)
                    .ok_or(err_custom_create!(
                        "Chain {} not found in config file",
                        get_dev_eth_options.chain_name
                    ))?;
            let cfg = chain_cfg
                .faucet_client
                .clone()
                .expect("No faucet client config found");
            let faucet_srv_prefix = cfg.faucet_srv;
            let faucet_lookup_domain = cfg.faucet_lookup_domain;
            let faucet_srv_port = cfg.faucet_srv_port;
            let faucet_host = cfg.faucet_host;

            faucet_donate(
                &faucet_srv_prefix,
                &faucet_lookup_domain,
                &faucet_host,
                faucet_srv_port,
                public_addr,
            )
            .await?;
        }
        PaymentCommands::MintTestTokens {
            mint_test_tokens_options,
        } => {
            log::info!("Generating test tokens...");
            let public_addr = if let Some(address) = mint_test_tokens_options.address {
                address
            } else if let Some(account_no) = mint_test_tokens_options.account_no {
                *public_addrs
                    .get(account_no)
                    .expect("No public adss found with specified account_no")
            } else {
                *public_addrs.first().expect("No public adss found")
            };
            let chain_cfg = config
                .chain
                .get(&mint_test_tokens_options.chain_name)
                .ok_or(err_custom_create!(
                    "Chain {} not found in config file",
                    mint_test_tokens_options.chain_name
                ))?;

            let payment_setup = PaymentSetup::new_empty(&config)?;
            let web3 = payment_setup.get_provider(chain_cfg.chain_id)?;

            mint_golem_token(
                web3,
                &conn.clone().unwrap(),
                chain_cfg.chain_id as u64,
                public_addr,
                chain_cfg.token.address,
                chain_cfg.mint_contract.clone().map(|c| c.address),
                true,
                chain_cfg.wrapper_contract.clone().map(|c| c.address),
            )
            .await?;
        }
        PaymentCommands::Attestation { attest } => match attest {
            AttestationCommands::Check { options } => {
                check_attestation_local(conn.clone().unwrap(), options, config).await?;
            }
        },
        PaymentCommands::Deposit { deposit } => match deposit {
            DepositCommands::Create {
                make_deposit_options,
            } => {
                make_deposit_local(
                    conn.clone().unwrap(),
                    make_deposit_options,
                    config,
                    &public_addrs,
                    signer,
                )
                .await?;
            }
            DepositCommands::Close {
                close_deposit_options,
            } => {
                close_deposit_local(
                    conn.clone().unwrap(),
                    close_deposit_options,
                    config,
                    &public_addrs,
                )
                .await?;
            }
            DepositCommands::Terminate {
                terminate_deposit_options,
            } => {
                terminate_deposit_local(
                    conn.clone().unwrap(),
                    terminate_deposit_options,
                    config,
                    &public_addrs,
                )
                .await?;
            }
            DepositCommands::Check {
                check_deposit_options,
            } => {
                deposit_details_local(check_deposit_options, config).await?;
            }
        },

        PaymentCommands::GenerateKey {
            generate_key_options,
        } => {
            log::info!("Generating private keys...");

            let res = gen_private_keys(generate_key_options.number_of_keys)?;

            for key in res.1.iter().enumerate() {
                println!("# ETH_ADDRESS_{}: {:#x}", key.0, key.1);
            }
            for key in res.0.iter().enumerate() {
                println!("# ETH_PRIVATE_KEY_{}: {}", key.0, key.1);
            }
            println!("ETH_PRIVATE_KEYS={}", res.0.join(","));
        }
        PaymentCommands::Transfer {
            single_transfer_options,
        } => {
            log::info!("Adding single transfer...");
            let chain_cfg = config
                .chain
                .get(&single_transfer_options.chain_name)
                .ok_or(err_custom_create!(
                    "Chain {} not found in config file",
                    single_transfer_options.chain_name
                ))?;

            #[allow(clippy::if_same_then_else)]
            let token = if single_transfer_options.token == "glm" {
                Some(format!("{:#x}", chain_cfg.token.address))
            } else if single_transfer_options.token == "eth" {
                None
            } else if single_transfer_options.token == "matic" {
                //matic is the same as eth
                None
            } else {
                return Err(err_custom_create!(
                    "Unknown token: {}",
                    single_transfer_options.token
                ));
            };

            let recipient = check_address_name(&single_transfer_options.recipient).unwrap();

            let public_addr = if let Some(address) = single_transfer_options.address {
                address
            } else if let Some(account_no) = single_transfer_options.account_no {
                *public_addrs
                    .get(account_no)
                    .expect("No public adss found with specified account_no")
            } else {
                *public_addrs.first().expect("No public address found")
            };
            //let mut db_transaction = conn.clone().unwrap().begin().await.unwrap();

            let amount_str = if let Some(amount) = single_transfer_options.amount {
                amount.to_u256_from_eth().unwrap().to_string()
            } else if single_transfer_options.all {
                let payment_setup = PaymentSetup::new_empty(&config)?;
                {
                    #[allow(clippy::if_same_then_else)]
                    if single_transfer_options.token == "glm" {
                        let args = GetBalanceArgs {
                            address: public_addr,
                            token_address: Some(chain_cfg.token.address),
                            call_with_details: chain_cfg.wrapper_contract.clone().map(|c| c.address),
                            block_number: None,
                            chain_id: Some(chain_cfg.chain_id as u64),
                        };
                        get_token_balance(
                            payment_setup.get_provider(chain_cfg.chain_id)?,
                            args
                        )
                        .await?.token_balance.ok_or(
                            err_custom_create!("No balance found for address {:#x}", public_addr)
                        )?
                        .to_string()
                    } else if single_transfer_options.token == "eth"
                        || single_transfer_options.token == "matic"
                    {
                        let val = payment_setup
                            .get_provider(chain_cfg.chain_id)?
                            .eth_balance(public_addr, None)
                            .await
                            .map_err(err_from!())?;
                        let gas_val = Decimal::from_str(&chain_cfg.max_fee_per_gas.to_string())
                            .map_err(|e| err_custom_create!("Failed to convert {e}"))?
                            * Decimal::from(21500); //leave some room for rounding error
                        let gas_val = gas_val.to_u256_from_gwei().map_err(err_from!())?;
                        if gas_val > val {
                            return Err(err_custom_create!(
                                "Not enough eth to pay for gas, required: {}, available: {}",
                                gas_val,
                                val
                            ));
                        }
                        (val - gas_val).to_string()
                    } else {
                        return Err(err_custom_create!(
                            "Unknown token: {}",
                            single_transfer_options.token
                        ));
                    }
                }
            } else {
                return Err(err_custom_create!("No amount specified"));
            };
            let amount_decimal = amount_str.to_eth().unwrap();

            let deposit_id_str = if let Some(deposit_id) = single_transfer_options.deposit_id {
                let lock_contract =
                    if let Some(lock_contract) = single_transfer_options.lock_contract {
                        lock_contract
                    } else {
                        chain_cfg
                            .lock_contract
                            .clone()
                            .map(|c| c.address)
                            .expect("No lock contract found")
                    };

                Some(
                    DepositId {
                        deposit_id: U256::from_str_radix(&deposit_id, 16)
                            .map_err(|e| err_custom_create!("Invalid deposit id: {}", e))?,
                        lock_address: lock_contract,
                    }
                    .to_db_string(),
                )
            } else {
                None
            };
            let mut tt = insert_token_transfer_with_deposit_check(
                &conn.clone().unwrap(),
                &TokenTransferDbObj {
                    id: 0,
                    payment_id: None,
                    from_addr: format!("{:#x}", public_addr),
                    receiver_addr: format!("{:#x}", recipient),
                    chain_id: chain_cfg.chain_id,
                    token_addr: token,
                    token_amount: amount_str,
                    deposit_id: deposit_id_str,
                    deposit_finish: 0,
                    create_date: Default::default(),
                    tx_id: None,
                    paid_date: None,
                    fee_paid: None,
                    error: None,
                },
            )
            .await
            .unwrap();

            let payment_id = format!("{}_transfer_{}", single_transfer_options.token, tt.id);
            tt.payment_id = Some(payment_id.clone());
            update_token_transfer(&conn.clone().unwrap(), &tt)
                .await
                .unwrap();

            log::info!(
                "Transfer added to db amount: {}, payment id: {}",
                amount_decimal,
                payment_id
            );
        }
        PaymentCommands::Balance {
            account_balance_options,
        } => {
            let mut account_balance_options = account_balance_options;
            if account_balance_options.accounts.is_none() {
                account_balance_options.accounts = Some(
                    public_addrs
                        .iter()
                        .map(|addr| format!("{:#x}", addr))
                        .collect::<Vec<String>>()
                        .join(","),
                );
            }

            let result = account_balance(account_balance_options, &config).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|err| err_custom_create!(
                    "Something went wrong when serializing to json {err}"
                ))?
            );
        }
        PaymentCommands::Generate { generate_options } => {
            if generate_options.append_to_db && cli.sqlite_read_only {
                return Err(err_custom_create!("Cannot append to db in read-only mode"));
            }
            generate_test_payments(
                generate_options,
                &config,
                public_addrs,
                Some(conn.clone().unwrap()),
            )
            .await?;
        }
        PaymentCommands::ExportHistory {
            export_history_stats_options,
        } => export_stats(conn.clone().unwrap(), export_history_stats_options, &config).await?,
        PaymentCommands::PaymentStats {
            payment_stats_options,
        } => run_stats(conn.clone().unwrap(), payment_stats_options, &config).await?,
        PaymentCommands::ScanBlockchain {
            scan_blockchain_options,
        } => scan_blockchain_local(conn.clone().unwrap(), scan_blockchain_options, config).await?,
        PaymentCommands::ImportPayments { import_options } => {
            log::info!("importing payments from file: {}", import_options.file);
            if !cli.sqlite_read_only {
                return Err(err_custom_create!(
                    "Cannot import payments in read-only mode"
                ));
            }
            let mut rdr = ReaderBuilder::new()
                .delimiter(import_options.separator as u8)
                .from_reader(std::fs::File::open(&import_options.file).map_err(err_from!())?);

            let deserialize = rdr.deserialize::<TokenTransferDbObj>();

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

                        if let Some(token_addr) = &token_transfer.token_addr {
                            if format!("{:#x}", chain_cfg.token.address)
                                != token_addr.to_lowercase()
                            {
                                return Err(err_custom_create!(
                                    "Token address in line {} is different from default token address {} != {:#x}",
                                    line_no,
                                    token_addr.to_lowercase(),
                                    chain_cfg.token.address
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
                insert_token_transfer(&conn.clone().unwrap(), &token_transfer)
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
        PaymentCommands::Cleanup { cleanup_options } => {
            if cleanup_options.remove_unsent_tx {
                let mut number_of_unsent_removed = 0;
                loop {
                    match remove_last_unsent_transactions(conn.clone().unwrap()).await {
                        Ok(Some(id)) => {
                            println!("Removed unsent transaction with id {}", id);
                            number_of_unsent_removed += 1;
                        }
                        Ok(None) => {
                            break;
                        }
                        Err(e) => {
                            return Err(err_custom_create!(
                                "Error when removing unsent transaction: {}",
                                e
                            ));
                        }
                    }
                }
                if number_of_unsent_removed == 0 {
                    println!("No unsent transactions found to remove");
                } else {
                    println!("Removed {} unsent transactions", number_of_unsent_removed);
                }
            }
            if cleanup_options.remove_tx_stuck {
                let mut transactions = get_next_transactions_to_process(
                    &conn.clone().unwrap(),
                    None,
                    1,
                    cleanup_options.chain_id.ok_or(err_custom_create!(
                        "Chain id not specified for stuck tx removal"
                    ))?,
                )
                .await
                .map_err(err_from!())?;

                let Some(tx) = transactions.get_mut(0) else {
                    println!("No transactions found to remove");
                    return Ok(());
                };
                if tx.first_stuck_date.is_some() {
                    match remove_transaction_force(&conn.clone().unwrap(), tx.id).await {
                        Ok(_) => {
                            println!(
                                "Removed stuck transaction with id {} (nonce: {})",
                                tx.id,
                                tx.nonce.unwrap_or(-1)
                            );
                        }
                        Err(e) => {
                            return Err(err_custom_create!(
                                "Error when removing transaction {}: {}",
                                tx.id,
                                e
                            ));
                        }
                    }
                } else {
                    println!("Transaction with id {} is not stuck, skipping", tx.id)
                }
            }
            if cleanup_options.remove_tx_unsafe {
                let mut transactions = get_next_transactions_to_process(
                    &conn.clone().unwrap(),
                    None,
                    1,
                    cleanup_options.chain_id.ok_or(err_custom_create!(
                        "Chain id not specified for unsafe tx removal"
                    ))?,
                )
                .await
                .map_err(err_from!())?;

                let Some(tx) = transactions.get_mut(0) else {
                    println!("No transactions found to remove");
                    return Ok(());
                };
                match remove_transaction_force(&conn.clone().unwrap(), tx.id).await {
                    Ok(_) => {
                        println!("Removed transaction with id {}", tx.id);
                    }
                    Err(e) => {
                        return Err(err_custom_create!(
                            "Error when removing transaction {}: {}",
                            tx.id,
                            e
                        ));
                    }
                }
            }
        }
        PaymentCommands::ShowConfig => {
            println!(
                "{}",
                toml::to_string_pretty(&config).map_err(|err| err_custom_create!(
                    "Something went wrong when serializing to json {err}"
                ))?
            );
        }
    }

    if let Some(conn) = conn.clone() {
        conn.close().await;
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
