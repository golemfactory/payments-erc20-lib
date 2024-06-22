use crate::signer::{Signer, SignerAccount};
use crate::transaction::{
    create_create_deposit, create_distribute_transaction, create_faucet_mint,
    create_terminate_deposit, create_token_transfer, find_receipt_extended, FindReceiptParseResult,
};
use crate::{err_custom_create, err_from};
use erc20_payment_lib_common::create_sqlite_connection;
use erc20_payment_lib_common::ops::{
    cleanup_allowance_tx, cleanup_token_transfer_tx, delete_tx, get_last_unsent_tx,
    get_token_transfers_by_deposit_id, get_transaction_chain, get_transactions,
    get_unpaid_token_transfers, insert_token_transfer, insert_token_transfer_with_deposit_check,
    insert_tx, update_token_transfer,
};
use std::collections::BTreeMap;
use std::ops::DerefMut;
use std::path::PathBuf;
use std::str::FromStr;

use crate::error::{ErrorBag, PaymentError};

use crate::setup::{ChainSetup, ExtraOptionsForTesting, PaymentSetup};

use crate::config::{self, Config};
use secp256k1::SecretKey;
use sqlx::SqlitePool;

use crate::account_balance::{test_balance_loop, BalanceOptions2};
use crate::config::AdditionalOptions;
use crate::contracts::CreateDepositArgs;
use crate::eth::{
    get_eth_addr_from_secret, get_latest_block_info, DepositDetails, GetBalanceArgs,
    GetBalanceResult,
};
use crate::sender::service_loop;
use crate::utils::{DecimalConvExt, StringConvExt, U256ConvExt};
use chrono::{DateTime, Utc};
use erc20_payment_lib_common::model::{DepositId, TokenTransferDbObj};
use erc20_payment_lib_common::{
    DriverEvent, DriverEventContent, FaucetData, SharedInfoTx, StatusProperty,
    TransactionStuckReason, Web3RpcPoolContent,
};
use erc20_rpc_pool::{Web3ExternalSources, Web3FullNodeData, Web3PoolType, Web3RpcPool};
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, Mutex, Notify};
use tokio::task::{JoinError, JoinHandle};
use web3::types::{Address, H256, U256};

#[derive(Debug, Clone, Serialize)]
pub struct SharedState {
    pub current_tx_info: BTreeMap<i64, SharedInfoTx>,
    //pub web3_rpc_pool: BTreeMap<i64, Vec<(Web3RpcParams, Web3RpcInfo)>>,
    #[serde(skip)]
    pub web3_pool_ref: Arc<std::sync::Mutex<BTreeMap<i64, Web3PoolType>>>,

    pub faucet: Option<FaucetData>,
    pub inserted: usize,
    pub idling: bool,

    pub accounts: Vec<SignerAccount>,
}

impl SharedState {
    pub fn set_tx_message(&mut self, id: i64, message: String) {
        if let Some(info) = self.current_tx_info.get_mut(&id) {
            info.message = message;
        } else {
            self.current_tx_info.insert(
                id,
                SharedInfoTx {
                    message,
                    error: None,
                    skip: false,
                },
            );
        }
    }

    pub fn set_tx_error(&mut self, id: i64, error: Option<String>) {
        if let Some(info) = self.current_tx_info.get_mut(&id) {
            info.error = error;
        } else {
            self.current_tx_info.insert(
                id,
                SharedInfoTx {
                    message: "".to_string(),
                    error,
                    skip: false,
                },
            );
        }
    }
    pub fn skip_tx(&mut self, id: i64) -> bool {
        if let Some(info) = self.current_tx_info.get_mut(&id) {
            info.skip = true;
            true
        } else {
            false
        }
    }
    pub fn is_skipped(&mut self, id: i64) -> bool {
        if let Some(info) = self.current_tx_info.get_mut(&id) {
            info.skip
        } else {
            false
        }
    }
    pub fn delete_tx_info(&mut self, id: i64) {
        self.current_tx_info.remove(&id);
    }
}

struct StatusTracker {
    status: Arc<Mutex<Vec<StatusProperty>>>,
}

impl StatusTracker {
    /// Add or update status_props so as to ensure that the given status_property is
    /// implied.
    ///
    /// Returns true if status_props was mutated, false otherwise
    fn update(status_props: &mut Vec<StatusProperty>, new_property: StatusProperty) -> bool {
        for old_property in status_props.iter_mut() {
            use StatusProperty::*;
            match (old_property, &new_property) {
                (
                    CantSign {
                        chain_id: id1,
                        address: addr1,
                    },
                    CantSign {
                        chain_id: id2,
                        address: addr2,
                    },
                ) if id1 == id2 && addr1 == addr2 => {
                    return false;
                }

                (
                    NoGas {
                        chain_id: id1,
                        address: addr1,
                        missing_gas: old_missing,
                    },
                    NoGas {
                        chain_id: id2,
                        address: addr2,
                        missing_gas: new_missing,
                    },
                ) if id1 == id2 && addr1 == addr2 => {
                    *old_missing = *new_missing;
                    return true;
                }

                (
                    NoToken {
                        chain_id: id1,
                        address: addr1,
                        missing_token: old_missing,
                    },
                    NoToken {
                        chain_id: id2,
                        address: addr2,
                        missing_token: new_missing,
                    },
                ) if id1 == id2 && addr1 == addr2 => {
                    *old_missing = *new_missing;
                    return true;
                }

                (
                    Web3RpcError {
                        chain_id: id1,
                        error: err1,
                    },
                    Web3RpcError {
                        chain_id: id2,
                        error: err2,
                    },
                ) if id1 == id2 => {
                    err1.clear();
                    err1.push_str(err2);
                    return true;
                }

                (TxStuck { chain_id: id1 }, TxStuck { chain_id: id2 }) if id1 == id2 => {
                    return false;
                }
                _ => {}
            }
        }

        status_props.push(new_property);
        true
    }

    /// Remove StatusProperty instances that are invalidated by
    /// a passing transaction with `ok_chain_id`
    ///
    /// Returns true if status_props was mutated, false otherwise
    fn clear_issues(status_props: &mut Vec<StatusProperty>, ok_chain_id: i64) -> bool {
        let old_len = status_props.len();

        #[allow(clippy::match_like_matches_macro)]
        status_props.retain(|s| match s {
            StatusProperty::CantSign { chain_id, .. } if *chain_id == ok_chain_id => false,
            StatusProperty::NoGas { chain_id, .. } if *chain_id == ok_chain_id => false,
            StatusProperty::NoToken { chain_id, .. } if *chain_id == ok_chain_id => false,
            StatusProperty::TxStuck { chain_id, .. } if *chain_id == ok_chain_id => false,
            StatusProperty::Web3RpcError { chain_id, .. } if *chain_id == ok_chain_id => false,
            _ => true,
        });

        status_props.len() != old_len
    }

    fn new(
        mut broadcast_sender: Option<broadcast::Sender<DriverEvent>>,
        mut mpsc_sender: Option<mpsc::Sender<DriverEvent>>,
        mut status_rx: mpsc::Receiver<DriverEvent>,
    ) -> Self {
        let status = Arc::new(Mutex::new(Vec::new()));
        let status_ = Arc::clone(&status);

        tokio::spawn(async move {
            let status = status_;
            while let Some(ev) = status_rx.recv().await {
                let mut pass_raw_message = true;
                let emit_changed = match &ev.content {
                    DriverEventContent::CantSign(details) => Self::update(
                        status.lock().await.deref_mut(),
                        StatusProperty::CantSign {
                            chain_id: details.chain_id(),
                            address: details.address().to_string(),
                        },
                    ),
                    DriverEventContent::TransactionStuck(TransactionStuckReason::NoGas(
                        details,
                    )) => {
                        let missing_gas = details.gas_needed - details.gas_balance;

                        Self::update(
                            status.lock().await.deref_mut(),
                            StatusProperty::NoGas {
                                chain_id: details.tx.chain_id,
                                address: details.tx.from_addr.clone(),
                                missing_gas,
                            },
                        )
                    }
                    DriverEventContent::TransactionStuck(TransactionStuckReason::NoToken(
                        details,
                    )) => {
                        let missing_token = details.token_balance - details.token_needed;
                        Self::update(
                            status.lock().await.deref_mut(),
                            StatusProperty::NoToken {
                                chain_id: details.tx.chain_id,
                                address: details.tx.from_addr.clone(),
                                missing_token,
                            },
                        )
                    }
                    DriverEventContent::TransactionStuck(TransactionStuckReason::GasPriceLow(
                        details,
                    )) => Self::update(
                        status.lock().await.deref_mut(),
                        StatusProperty::TxStuck {
                            chain_id: details.tx.chain_id,
                        },
                    ),
                    DriverEventContent::TransferFinished(transaction_finished_info) => {
                        Self::clear_issues(
                            status.lock().await.deref_mut(),
                            transaction_finished_info.token_transfer_dao.chain_id,
                        )
                    }
                    DriverEventContent::Web3RpcMessage(rpc_pool_info) => {
                        match &rpc_pool_info.content {
                            Web3RpcPoolContent::Success => {
                                //Self::clear_issues(status.lock().await.deref_mut(), rpc_pool_info.chain_id)
                                pass_raw_message = false;
                                false
                            }
                            Web3RpcPoolContent::Error(err) => Self::update(
                                status.lock().await.deref_mut(),
                                StatusProperty::Web3RpcError {
                                    chain_id: rpc_pool_info.chain_id as i64,
                                    error: err.clone(),
                                },
                            ),
                            Web3RpcPoolContent::AllEndpointsFailed => Self::update(
                                status.lock().await.deref_mut(),
                                StatusProperty::Web3RpcError {
                                    chain_id: rpc_pool_info.chain_id as i64,
                                    error: "All endpoints failed".to_string(),
                                },
                            ),
                        }
                    }

                    _ => false,
                };

                if pass_raw_message {
                    if let Some(sender) = &mut mpsc_sender {
                        if let Err(err) = sender.send(ev.clone()).await {
                            log::warn!("Error resending driver event: {}", err);
                        }
                        if emit_changed {
                            if let Err(err) = sender
                                .send(DriverEvent::now(DriverEventContent::StatusChanged(
                                    status.lock().await.clone(),
                                )))
                                .await
                            {
                                log::warn!("Error resending driver status changed event: {}", err);
                            }
                        }
                    }

                    if let Some(sender) = &mut broadcast_sender {
                        if let Err(_err) = sender.send(ev) {
                            //channel closed - it's normal
                        }
                        if emit_changed {
                            if let Err(_err) = sender.send(DriverEvent::now(
                                DriverEventContent::StatusChanged(status.lock().await.clone()),
                            )) {
                                //channel closed - it's normal
                            }
                        }
                    }
                }
            }
            log::debug!("Status tracker finished");
        });

        StatusTracker { status }
    }

    async fn get_status(&self) -> Vec<StatusProperty> {
        self.status.lock().await.clone()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TransferType {
    Token,
    Gas,
}

pub struct PaymentRuntime {
    //pub runtime_handles: Arc<std::sync::Mutex<Vec<JoinHandle<()>>>>,
    pub setup: PaymentSetup,
    pub shared_state: Arc<std::sync::Mutex<SharedState>>,
    pub wake: Arc<Notify>,
    pub driver_broadcast_sender: Option<broadcast::Sender<DriverEvent>>,
    pub driver_mpsc_sender: Option<mpsc::Sender<DriverEvent>>,
    pub raw_event_sender: mpsc::Sender<DriverEvent>,
    conn: SqlitePool,
    status_tracker: StatusTracker,
    config: Config,
}

pub struct PaymentRuntimeArgs {
    pub secret_keys: Vec<SecretKey>,
    pub db_filename: PathBuf,
    pub config: config::Config,
    pub conn: Option<SqlitePool>,
    pub options: Option<AdditionalOptions>,
    pub broadcast_sender: Option<broadcast::Sender<DriverEvent>>,
    pub mspc_sender: Option<mpsc::Sender<DriverEvent>>,
    pub extra_testing: Option<ExtraOptionsForTesting>,
}

#[derive(Debug, Clone)]
pub struct TransferArgs {
    pub network: String,
    pub from: Address,
    pub receiver: Address,
    pub tx_type: TransferType,
    pub amount: U256,
    pub payment_id: String,
    pub deadline: Option<DateTime<Utc>>,
    pub deposit_id: Option<DepositId>,
}

impl PaymentRuntime {
    fn start_service_loop(
        &self,
        signer_address: Address,
        chain_id: i64,
        notify: Arc<Notify>,
        extra_testing: Option<ExtraOptionsForTesting>,
        options: AdditionalOptions,
    ) -> JoinHandle<()> {
        let shared_state_clone = self.shared_state.clone();
        let raw_event_sender = self.raw_event_sender.clone();
        let config = self.config.clone();
        let ps = self.setup.clone();
        let conn = self.conn.clone();
        let jh = tokio::task::spawn(async move {
            if let Some(balance_check_loop) =
                extra_testing.clone().and_then(|e| e.balance_check_loop)
            {
                if config.chain.values().len() != 1 {
                    panic!("balance_check_loop can be used only with single chain");
                }
                let config_chain = config.chain.values().next().unwrap().clone();
                let balance_options = BalanceOptions2 {
                    interval: Some(2.0),
                    debug_loop: Some(balance_check_loop),
                };
                match test_balance_loop(
                    Some(shared_state_clone),
                    ps.clone(),
                    balance_options,
                    &config_chain,
                )
                .await
                {
                    Ok(_) => {
                        log::info!("Balance debug loop finished");
                    }
                    Err(e) => {
                        log::error!("Balance debug loop finished with error: {}", e);
                        panic!("Balance debug loop finished with error: {}", e);
                    }
                }
                return;
            }
            if options.skip_service_loop && options.keep_running {
                log::warn!("Started with skip_service_loop and keep_running, no transaction will be sent or processed");
                loop {
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                }
            } else {
                service_loop(
                    shared_state_clone,
                    chain_id,
                    signer_address,
                    notify,
                    &conn,
                    &ps,
                    Some(raw_event_sender),
                )
                .await
            }
        });
        jh
    }

    pub async fn new(
        payment_runtime_args: PaymentRuntimeArgs,
        signer: Arc<Box<dyn Signer + Send + Sync + 'static>>,
    ) -> Result<PaymentRuntime, PaymentError> {
        let options = payment_runtime_args.options.unwrap_or_default();

        let web3_rpc_pool_info =
            Arc::new(std::sync::Mutex::new(BTreeMap::<i64, Web3PoolType>::new()));

        let (raw_event_sender, status_rx) = tokio::sync::mpsc::channel::<DriverEvent>(1);

        let mut payment_setup = PaymentSetup::new(
            &payment_runtime_args.config,
            &options,
            web3_rpc_pool_info.clone(),
            Some(raw_event_sender.clone()),
        )?;
        payment_setup.use_transfer_for_single_payment = options.use_transfer_for_single_payment;
        payment_setup
            .extra_options_for_testing
            .clone_from(&payment_runtime_args.extra_testing);
        payment_setup.contract_use_direct_method = options.contract_use_direct_method;
        payment_setup.contract_use_unpacked_method = options.contract_use_unpacked_method;
        log::debug!("Starting payment engine: {:#?}", payment_setup);

        let conn = if let Some(conn) = payment_runtime_args.conn {
            conn
        } else {
            log::info!(
                "connecting to sqlite file db: {}",
                payment_runtime_args.db_filename.display()
            );
            create_sqlite_connection(Some(&payment_runtime_args.db_filename), None, false, true)
                .await?
        };

        let driver_broadcast_sender = payment_runtime_args.broadcast_sender.clone();
        let driver_mpsc_sender = payment_runtime_args.mspc_sender.clone();

        let status_tracker = StatusTracker::new(
            payment_runtime_args.broadcast_sender,
            payment_runtime_args.mspc_sender,
            status_rx,
        );

        let accounts = payment_runtime_args
            .secret_keys
            .iter()
            .map(|s| SignerAccount::new(get_eth_addr_from_secret(s), signer.clone()))
            .collect::<Vec<SignerAccount>>();

        let shared_state = Arc::new(std::sync::Mutex::new(SharedState {
            accounts: vec![],
            inserted: 0,
            idling: false,
            current_tx_info: BTreeMap::new(),
            faucet: None,
            web3_pool_ref: web3_rpc_pool_info.clone(),
        }));

        let notify = Arc::new(Notify::new());

        let pr = PaymentRuntime {
            //runtime_handles: Arc::new(std::sync::Mutex::new(Vec::new())),
            setup: payment_setup,
            shared_state,
            wake: notify.clone(),
            conn,
            status_tracker,
            driver_broadcast_sender,
            driver_mpsc_sender,
            raw_event_sender,
            config: payment_runtime_args.config,
        };

        for signer_account in accounts {
            pr.add_account(
                signer_account,
                payment_runtime_args.extra_testing.clone(),
                options.clone(),
            );
        }

        /* - use this to test notifies
        let notify_ = notify.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(fastrand::u64(1..20))).await;
                notify_.notify_one();
            }
        });
         */

        Ok(pr)
    }

    fn get_and_remove_tasks(&self) -> Vec<JoinHandle<()>> {
        let mut task_handles = Vec::new();
        let mut lock_shared_state = self.shared_state.lock().unwrap();

        //this shouldn't end in deadlock. It just extracts all handles and removes them from the lists
        for account in lock_shared_state.accounts.iter_mut() {
            for jh in account.jh.lock().unwrap().iter_mut() {
                if let Some(jh) = jh.take() {
                    task_handles.push(jh);
                }
            }
        }

        task_handles
    }

    pub fn is_any_task_running(&self) -> bool {
        let lock_shared_state = self.shared_state.lock().unwrap();

        for account in lock_shared_state.accounts.iter() {
            for jh in account.jh.lock().unwrap().iter().flatten() {
                if !jh.is_finished() {
                    return true;
                }
            }
        }
        false
    }

    pub fn is_any_task_finished(&self) -> bool {
        let lock_shared_state = self.shared_state.lock().unwrap();

        for account in lock_shared_state.accounts.iter() {
            for jh in account.jh.lock().unwrap().iter().flatten() {
                if jh.is_finished() {
                    return true;
                }
            }
        }
        false
    }

    pub async fn join_tasks(&self) -> Result<(), JoinError> {
        let handles = self.get_and_remove_tasks();
        for handle in handles {
            match handle.await {
                Ok(_) => {}
                Err(e) => {
                    log::error!("Task finished with error: {}", e);
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    pub fn abort_tasks(&self) {
        let handles = self.get_and_remove_tasks();
        for handle in handles {
            handle.abort();
        }
    }

    pub fn add_account(
        &self,
        payment_account: SignerAccount,
        extra_testing: Option<ExtraOptionsForTesting>,
        options: AdditionalOptions,
    ) -> bool {
        log::debug!("Adding account: {}", payment_account);
        let mut sh = self.shared_state.lock().unwrap();

        if sh
            .accounts
            .iter()
            .any(|a| a.address == payment_account.address)
        {
            log::error!("Account already added: {}", payment_account);
            return false;
        }
        for chain_id in self.chains() {
            log::debug!(
                "Starting service loop for account: {} and chain id: {}",
                payment_account.address,
                chain_id
            );
            let jh = self.start_service_loop(
                payment_account.address,
                chain_id,
                self.wake.clone(),
                extra_testing.clone(),
                options.clone(),
            );
            payment_account.jh.lock().as_mut().unwrap().push(Some(jh));
        }
        sh.accounts.push(payment_account);

        true
    }

    pub async fn get_unpaid_token_amount(
        &self,
        chain_name: String,
        sender: Address,
    ) -> Result<U256, PaymentError> {
        let chain_cfg = self
            .config
            .chain
            .get(&chain_name)
            .ok_or(err_custom_create!(
                "Chain {} not found in config file",
                chain_name
            ))?;
        get_unpaid_token_amount(
            &self.conn,
            chain_cfg.chain_id,
            chain_cfg.token.address,
            sender,
        )
        .await
    }

    pub async fn validate_deposit(
        &self,
        chain_name: String,
        deposit_id: DepositId,
        validate_args: BTreeMap<String, String>,
    ) -> Result<ValidateDepositResult, PaymentError> {
        let chain_cfg = self
            .config
            .chain
            .get(&chain_name)
            .ok_or(err_custom_create!(
                "Chain {} not found in config file",
                chain_name
            ))?;

        let web3 = self.setup.get_provider(chain_cfg.chain_id)?;

        validate_deposit(web3, deposit_id, validate_args).await
    }

    pub async fn deposit_details(
        &self,
        chain_name: String,
        deposit_id: DepositId,
    ) -> Result<DepositDetails, PaymentError> {
        let chain_cfg = self
            .config
            .chain
            .get(&chain_name)
            .ok_or(err_custom_create!(
                "Chain {} not found in config file",
                chain_name
            ))?;

        let web3 = self.setup.get_provider(chain_cfg.chain_id)?;

        deposit_details(web3, deposit_id).await
    }

    pub async fn get_token_balance(
        &self,
        chain_name: String,
        address: Address,
        block_number: Option<u64>,
    ) -> Result<GetBalanceResult, PaymentError> {
        let chain_cfg = self
            .config
            .chain
            .get(&chain_name)
            .ok_or(err_custom_create!(
                "Chain {} not found in config file",
                chain_name
            ))?;

        let token_address = chain_cfg.token.address;

        let web3 = self.setup.get_provider(chain_cfg.chain_id)?;

        let args = GetBalanceArgs {
            address,
            token_address: Some(token_address),
            call_with_details: chain_cfg.wrapper_contract.clone().map(|c| c.address),
            block_number,
            chain_id: Some(chain_cfg.chain_id as u64),
        };
        get_token_balance(web3, args).await
    }

    /// Force sources and enpoints check depending on input. If wait is set to false, it is nonblocking.
    pub async fn force_check_endpoint_info(
        &self,
        network: Option<String>,
        resolve: bool,
        verify: bool,
        wait: bool,
    ) -> Result<(), PaymentError> {
        //do not keep locks

        let chain_cfgs = if let Some(network) = network {
            vec![self
                .config
                .chain
                .get(&network)
                .ok_or(err_custom_create!(
                    "Chain {} not found in config file",
                    network
                ))?
                .clone()]
        } else {
            self.config.chain.values().cloned().collect()
        };

        for chain_cfg in chain_cfgs {
            let web3 = self.setup.get_provider(chain_cfg.chain_id)?;

            if resolve {
                let jh = web3
                    .external_sources_resolver
                    .clone()
                    .start_resolve_if_needed(web3.clone(), true);
                if wait {
                    jh.unwrap().await.map_err(|e| {
                        err_custom_create!("Error waiting for external resolver: {}", e)
                    })?
                }
            }

            if verify {
                web3.endpoint_verifier
                    .start_verify_if_needed(web3.clone(), true);
                if wait {
                    let vh = web3.endpoint_verifier.get_join_handle();
                    if let Some(vh) = vh {
                        vh.await.map_err(|e| {
                            err_custom_create!("Error waiting for endpoint verifier: {}", e)
                        })?
                    }
                }
            }
        }
        Ok(())
    }

    pub fn get_rpc_sources(
        &self,
        network: Option<String>,
    ) -> Result<BTreeMap<String, Web3ExternalSources>, PaymentError> {
        let chain_cfgs = if let Some(network) = network {
            vec![(
                network.clone(),
                self.config
                    .chain
                    .get(&network)
                    .ok_or(err_custom_create!(
                        "Chain {} not found in config file",
                        network
                    ))?
                    .clone(),
            )]
        } else {
            self.config
                .chain
                .iter()
                .map(|el| (el.0.clone(), el.1.clone()))
                .collect()
        };

        let mut res: BTreeMap<String, Web3ExternalSources> = BTreeMap::new();
        for chain_cfg in chain_cfgs {
            let web3 = self.setup.get_provider(chain_cfg.1.chain_id)?;
            res.insert(
                chain_cfg.0,
                Web3ExternalSources {
                    json_sources: web3.external_json_sources.clone(),
                    dns_sources: web3.external_dns_sources.clone(),
                },
            );
        }

        Ok(res)
    }

    pub fn get_rpc_endpoints(
        &self,
        network: Option<String>,
    ) -> Result<BTreeMap<String, Vec<Web3FullNodeData>>, PaymentError> {
        let my_data = self.shared_state.lock().unwrap();
        // Convert BTreeMap of Arenas to BTreeMap of Vec because serde can't serialize Arena
        let web3_rpc_pool_info = my_data
            .web3_pool_ref
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| {
                network.is_none()
                    || self.setup.chain_setup.get(k).map(|s| s.network.clone()) == network
            })
            .map(|(k, v)| {
                (
                    self.setup
                        .chain_setup
                        .get(k)
                        .map(|s| s.network.clone())
                        .unwrap_or("unknown".to_string()),
                    v.try_lock_for(Duration::from_secs(5))
                        .unwrap()
                        .iter()
                        .map(|pair| {
                            let v = pair.1.clone();
                            let val = v.try_read_for(Duration::from_secs(5)).unwrap().clone();
                            Web3FullNodeData {
                                params: val.web3_rpc_params,
                                info: val.web3_rpc_info,
                            }
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>();

        Ok(web3_rpc_pool_info)
    }

    pub async fn transfer_guess_account(
        &self,
        transfer_args: TransferArgs,
    ) -> Result<(), PaymentError> {
        let account = {
            self.shared_state
                .lock()
                .unwrap()
                .accounts
                .iter()
                .find(|a| a.address == transfer_args.from)
                .cloned()
        };
        if let Some(account) = account {
            self.transfer_with_account(&account, transfer_args).await
        } else {
            Err(err_custom_create!(
                "Account {:#x} not found in active accounts",
                transfer_args.from
            ))
        }
    }

    pub async fn transfer_with_account(
        &self,
        account: &SignerAccount,
        transfer_args: TransferArgs,
    ) -> Result<(), PaymentError> {
        let chain_cfg = self
            .config
            .chain
            .get(&transfer_args.network)
            .ok_or(err_custom_create!(
                "Chain {} not found in config file",
                transfer_args.network
            ))?;

        let token_addr = match transfer_args.tx_type {
            TransferType::Token => {
                let address = chain_cfg.token.address;
                Some(address)
            }
            TransferType::Gas => None,
        };

        let token_transfer = create_token_transfer(
            transfer_args.from,
            transfer_args.receiver,
            chain_cfg.chain_id,
            Some(&transfer_args.payment_id),
            token_addr,
            transfer_args.amount,
            transfer_args.deposit_id,
        );

        insert_token_transfer_with_deposit_check(&self.conn, &token_transfer).await?;

        if !self.setup.ignore_deadlines {
            if let Some(deadline) = transfer_args.deadline {
                let mut ext_gath_time_guard = account.external_gather_time.lock().unwrap();
                let new_time = ext_gath_time_guard
                    .map(|t| t.min(deadline))
                    .unwrap_or(deadline);

                if Some(new_time) != *ext_gath_time_guard {
                    *ext_gath_time_guard = Some(new_time);
                    self.wake.notify_one();
                }
            }
        }

        Ok(())
    }

    pub async fn distribute_gas(
        &self,
        chain_name: &str,
        from: Address,
    ) -> Result<(), PaymentError> {
        let chain_cfg = self.config.chain.get(chain_name).ok_or(err_custom_create!(
            "Chain {} not found in config file",
            chain_name
        ))?;
        let golem_address = chain_cfg.token.address;
        let web3 = self.setup.get_provider(chain_cfg.chain_id)?;

        let res = mint_golem_token(
            web3,
            &self.conn,
            chain_cfg.chain_id as u64,
            from,
            golem_address,
            chain_cfg.mint_contract.clone().map(|c| c.address),
            false,
            chain_cfg.wrapper_contract.clone().map(|c| c.address),
        )
        .await;
        self.wake.notify_one();
        res
    }

    pub async fn mint_golem_token(
        &self,
        chain_name: &str,
        from: Address,
    ) -> Result<(), PaymentError> {
        let chain_cfg = self.config.chain.get(chain_name).ok_or(err_custom_create!(
            "Chain {} not found in config file",
            chain_name
        ))?;
        let golem_address = chain_cfg.token.address;
        let web3 = self.setup.get_provider(chain_cfg.chain_id)?;

        let res = mint_golem_token(
            web3,
            &self.conn,
            chain_cfg.chain_id as u64,
            from,
            golem_address,
            chain_cfg.mint_contract.clone().map(|c| c.address),
            false,
            chain_cfg.wrapper_contract.clone().map(|c| c.address),
        )
        .await;
        self.wake.notify_one();
        res
    }

    pub async fn get_status(&self) -> Vec<StatusProperty> {
        self.status_tracker.get_status().await
    }

    pub fn get_chain(&self, chain_id: i64) -> Option<&ChainSetup> {
        self.setup.chain_setup.get(&chain_id)
    }

    pub fn network_name(&self, chain_id: i64) -> Option<&str> {
        self.get_chain(chain_id).map(|chain| chain.network.as_str())
    }

    pub async fn verify_transaction(
        &self,
        chain_id: i64,
        tx_hash: H256,
        sender: Address,
        receiver: Address,
        amount: U256,
    ) -> Result<VerifyTransactionResult, PaymentError> {
        let _ = self.network_name(chain_id).ok_or(err_custom_create!(
            "Chain {} not found in config file",
            chain_id
        ))?;
        let glm_address = self
            .get_chain(chain_id)
            .ok_or(err_custom_create!("Chain {} not found", chain_id))?
            .glm_address;
        let prov = self.setup.get_provider(chain_id)?;
        verify_transaction(
            prov,
            chain_id,
            tx_hash,
            sender,
            receiver,
            amount,
            glm_address,
        )
        .await
    }

    pub async fn close_deposit(
        &self,
        chain_name: &str,
        from: Address,
        deposit_id: DepositId,
    ) -> Result<(), PaymentError> {
        let chain_cfg = self.config.chain.get(chain_name).ok_or(err_custom_create!(
            "Chain {} not found in config file",
            chain_name
        ))?;

        let web3 = self.setup.get_provider(chain_cfg.chain_id)?;
        close_deposit(
            web3,
            &self.conn,
            chain_cfg.chain_id as u64,
            from,
            CloseDepositOptionsInt {
                skip_deposit_check: true,
                deposit_id,
                token_address: chain_cfg.token.address,
            },
        )
        .await?;

        Ok(())
    }

    pub fn chains(&self) -> Vec<i64> {
        self.setup.chain_setup.keys().copied().collect()
    }
}

pub enum VerifyTransactionResult {
    Verified { amount: U256 },
    Rejected(String),
}

impl VerifyTransactionResult {
    pub fn verified(&self) -> bool {
        matches!(self, Self::Verified { .. })
    }

    pub fn rejected(&self) -> bool {
        matches!(self, Self::Rejected { .. })
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn distribute_gas(
    web3: Arc<Web3RpcPool>,
    conn: &SqlitePool,
    chain_id: u64,
    from: Address,
    distribute_contract_address: Option<Address>,
    skip_balance_check: bool,
    recipients: &[Address],
    amounts: &[rust_decimal::Decimal],
) -> Result<(), PaymentError> {
    let distribute_contract_address =
        if let Some(distribute_contract_address) = distribute_contract_address {
            distribute_contract_address
        } else {
            return Err(err_custom_create!(
                "Distribute contract address unknown. If not sure try on holesky network"
            ));
        };

    if recipients.len() != amounts.len() {
        return Err(err_custom_create!(
            "recipients and amounts must have the same length"
        ));
    }

    let mut amounts_u256: Vec<U256> = Vec::with_capacity(amounts.len());

    let mut sum_u256 = U256::zero();
    for amount in amounts {
        let amount = amount
            .to_u256_from_eth()
            .map_err(|err| err_custom_create!("Invalid amount: {} - {}", amount, err))?;
        amounts_u256.push(amount);
        sum_u256 += amount;
    }

    if !skip_balance_check {
        //todo check if we have enough gas + token to distribute
        let get_eth_balance = web3
            .clone()
            .eth_balance(from, None)
            .await
            .map_err(err_from!())?
            .to_eth_saturate();

        if get_eth_balance < Decimal::from_f64(0.000001).unwrap() {
            return Err(err_custom_create!(
                "You need at least 0.000001 ETH to continue. You have {} ETH on network with chain id: {} and account {:#x} ",
                get_eth_balance,
                chain_id,
                from
            ));
        }
    }

    let distribute_tx = create_distribute_transaction(
        from,
        distribute_contract_address,
        chain_id,
        None,
        recipients,
        &amounts_u256,
    )?;
    let distribute_tx = insert_tx(conn, &distribute_tx).await.map_err(err_from!())?;

    log::info!(
        "Distribute transaction added to queue: {}",
        distribute_tx.id
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn mint_golem_token(
    web3: Arc<Web3RpcPool>,
    conn: &SqlitePool,
    chain_id: u64,
    from: Address,
    glm_address: Address,
    faucet_contract_address: Option<Address>,
    skip_balance_check: bool,
    wrapper_contract_address: Option<Address>,
) -> Result<(), PaymentError> {
    let faucet_contract_address = if let Some(faucet_contract_address) = faucet_contract_address {
        faucet_contract_address
    } else {
        return Err(err_custom_create!(
            "Faucet/mint contract address unknown. If not sure try on holesky network"
        ));
    };

    if !skip_balance_check {
        let args = GetBalanceArgs {
            address: from,
            token_address: Some(glm_address),
            call_with_details: wrapper_contract_address,
            block_number: None,
            chain_id: Some(chain_id),
        };

        let token_balance_result = get_token_balance(web3.clone(), args).await?;

        let balance = token_balance_result
            .gas_balance
            .ok_or(err_custom_create!("Gas balance not found"))?
            .to_eth_saturate();
        if balance < Decimal::from_f64(0.005).unwrap() {
            return Err(err_custom_create!(
                "You need at least 0.005 ETH to continue. You have {} ETH on network with chain id: {} and account {:#x} ",
                balance,
                chain_id,
                from
            ));
        }

        let token_balance = token_balance_result
            .token_balance
            .ok_or(err_custom_create!("Token balance not found"))?
            .to_eth_saturate();

        if token_balance > Decimal::from_f64(500.0).unwrap() {
            return Err(err_custom_create!(
                "You already have {} tGLM on network with chain id: {} and account {:#x} ",
                token_balance,
                chain_id,
                from
            ));
        };
    }

    let mut db_transaction = conn.begin().await.map_err(err_from!())?;
    let filter = "method=\"FAUCET.create\" AND fee_paid is NULL";

    let tx_existing = get_transactions(
        &mut *db_transaction,
        Some(from),
        Some(filter),
        None,
        None,
        Some(chain_id as i64),
    )
    .await
    .map_err(err_from!())?;

    if let Some(tx) = tx_existing.first() {
        return Err(err_custom_create!(
            "You already have a pending mint (create) transaction with id: {}",
            tx.id
        ));
    }

    let faucet_mint_tx = create_faucet_mint(from, faucet_contract_address, chain_id, None)?;
    let mint_tx = insert_tx(&mut *db_transaction, &faucet_mint_tx)
        .await
        .map_err(err_from!())?;
    db_transaction.commit().await.map_err(err_from!())?;

    log::info!("Mint transaction added to queue: {}", mint_tx.id);
    Ok(())
}

pub async fn deposit_details(
    web3: Arc<Web3RpcPool>,
    deposit_id: DepositId,
) -> Result<DepositDetails, PaymentError> {
    let block_info = get_latest_block_info(web3.clone()).await?;

    let mut result = crate::eth::get_deposit_details(
        web3.clone(),
        deposit_id.deposit_id,
        deposit_id.lock_address,
        Some(block_info.block_number),
    )
    .await?;
    result.current_block_datetime = Some(block_info.block_date);
    if result.spender.is_zero() && result.funder.is_zero() {
        return Ok(result);
    }

    Ok(result)
}

pub enum ValidateDepositResult {
    Valid,
    Invalid(String),
}

pub async fn validate_deposit(
    web3: Arc<Web3RpcPool>,
    deposit_id: DepositId,
    validate_args: BTreeMap<String, String>,
) -> Result<ValidateDepositResult, PaymentError> {
    let block_info = get_latest_block_info(web3.clone()).await?;

    log::warn!("Validating deposit: {:?}", validate_args);
    crate::eth::validate_deposit_eth(
        web3.clone(),
        deposit_id.deposit_id,
        deposit_id.lock_address,
        validate_args,
        Some(block_info.block_number),
    )
    .await
}

pub struct CloseDepositOptionsInt {
    pub skip_deposit_check: bool,
    pub deposit_id: DepositId,
    pub token_address: Address,
}

pub struct TerminateDepositOptionsInt {
    pub skip_deposit_check: bool,
    pub deposit_id: DepositId,
}

pub async fn close_deposit(
    web3: Arc<Web3RpcPool>,
    conn: &SqlitePool,
    chain_id: u64,
    from: Address,
    opt: CloseDepositOptionsInt,
) -> Result<(), PaymentError> {
    //let mut block_info: Option<Web3BlockInfo> = None;
    if !opt.skip_deposit_check {
        let deposit_details = deposit_details(web3.clone(), opt.deposit_id).await?;
        if deposit_details.amount_decimal.is_zero() {
            log::error!("Deposit {} not found", opt.deposit_id.deposit_id);

            return Err(err_custom_create!(
                "Deposit {} not found",
                opt.deposit_id.deposit_id
            ));
        }
        if deposit_details.spender != from {
            log::error!(
                "You are not the spender of deposit {}",
                opt.deposit_id.deposit_id
            );
            return Err(err_custom_create!(
                "You are not the spender of deposit {}",
                opt.deposit_id.deposit_id
            ));
        }
    }

    let mut db_transaction = conn.begin().await.map_err(err_from!())?;

    let current_token_transfers = get_token_transfers_by_deposit_id(
        &mut *db_transaction,
        chain_id as i64,
        &opt.deposit_id.to_db_string(),
    )
    .await
    .map_err(err_from!())?;

    for tt in &current_token_transfers {
        if tt.deposit_finish > 0 {
            return Err(err_custom_create!(
                "Deposit {} already being closed or closed",
                opt.deposit_id.deposit_id
            ));
        }
    }
    let mut candidate_for_mark_close: Option<&TokenTransferDbObj> = None;
    for tt in &current_token_transfers {
        if tt.tx_id.is_none() {
            if let Some(old_tx) = candidate_for_mark_close {
                if old_tx.id < tt.id {
                    candidate_for_mark_close = Some(tt);
                } else {
                    candidate_for_mark_close = Some(old_tx);
                }
            } else {
                candidate_for_mark_close = Some(tt);
            }
        }
    }

    if let Some(tt) = candidate_for_mark_close {
        let mut tt = tt.clone();
        tt.deposit_finish = 1;
        update_token_transfer(&mut *db_transaction, &tt)
            .await
            .map_err(err_from!())?;
    } else {
        //empty transfer is just a marker that we need deposit to be closed
        let new_tt = TokenTransferDbObj {
            id: 0,
            payment_id: Some(format!("close_deposit_{:#x}", opt.deposit_id.deposit_id)),
            from_addr: format!("{:#x}", from),
            receiver_addr: format!("{:#x}", Address::zero()),
            chain_id: chain_id as i64,
            token_addr: Some(format!("{:#x}", opt.token_address)),
            token_amount: "0".to_string(),
            deposit_id: Some(opt.deposit_id.to_db_string()),
            deposit_finish: 1,
            create_date: chrono::Utc::now(),
            tx_id: None,
            paid_date: None,
            fee_paid: None,
            error: None,
        };
        insert_token_transfer(&mut *db_transaction, &new_tt)
            .await
            .map_err(err_from!())?;
    }

    //let mut db_transaction = conn.begin().await.map_err(err_from!())?;
    //let make_deposit_tx = insert_tx(&mut *db_transaction, &free_deposit_tx_id)
    //    .await
    //    .map_err(err_from!())?;
    db_transaction.commit().await.map_err(err_from!())?;

    //log::info!("Close deposit added to queue: {}", make_deposit_tx.id);
    Ok(())
}

pub async fn terminate_deposit(
    web3: Arc<Web3RpcPool>,
    conn: &SqlitePool,
    chain_id: u64,
    from: Address,
    opt: TerminateDepositOptionsInt,
) -> Result<(), PaymentError> {
    //let mut block_info: Option<Web3BlockInfo> = None;
    if !opt.skip_deposit_check {
        let deposit_id = opt.deposit_id;
        let deposit_details = deposit_details(web3.clone(), deposit_id).await?;
        if deposit_details.amount_decimal.is_zero() {
            log::error!("Deposit {} not found", deposit_id.deposit_id);

            return Err(err_custom_create!(
                "Deposit {} not found",
                deposit_id.deposit_id
            ));
        }
        if deposit_details.funder != from {
            log::error!(
                "You are not the funder of deposit {}",
                opt.deposit_id.deposit_id
            );
            return Err(err_custom_create!(
                "You are not the funder of deposit {}",
                opt.deposit_id.deposit_id
            ));
        }
        let est_time_left = (deposit_details.valid_to - Utc::now()).num_seconds();

        if est_time_left > 10 {
            log::error!(
                "Deposit {} is not ready to be terminated. Estimated time left: {}",
                deposit_id.deposit_id,
                est_time_left
            );
            return Err(err_custom_create!(
                "Deposit {} is not ready to be terminated. Estimated time left: {}",
                deposit_id.deposit_id,
                est_time_left
            ));
        }
    }
    let free_deposit_tx_id = create_terminate_deposit(
        from,
        opt.deposit_id.lock_address,
        chain_id,
        None,
        opt.deposit_id.nonce(),
    )?;

    let mut db_transaction = conn.begin().await.map_err(err_from!())?;
    let make_deposit_tx = insert_tx(&mut *db_transaction, &free_deposit_tx_id)
        .await
        .map_err(err_from!())?;
    db_transaction.commit().await.map_err(err_from!())?;

    log::info!("Terminate deposit added to queue: {}", make_deposit_tx.id);
    Ok(())
}

pub struct CreateDepositOptionsInt {
    pub lock_contract_address: Address,
    pub spender: Address,
    pub skip_balance_check: bool,
    pub amount: Option<Decimal>,
    pub fee_amount: Option<Decimal>,
    pub allocate_all: bool,
    pub deposit_nonce: u64,
    pub timestamp: u64,
}

pub async fn make_deposit(
    web3: Arc<Web3RpcPool>,
    conn: &SqlitePool,
    chain_id: u64,
    from: Address,
    glm_address: Address,
    opt: CreateDepositOptionsInt,
) -> Result<(), PaymentError> {
    let amount = if let Some(amount) = opt.amount {
        amount.to_u256_from_eth().map_err(err_from!())?
    } else {
        return Err(err_custom_create!("Amount not specified. Use --amount"));
    };
    let fee_amount = if let Some(fee_amount) = opt.fee_amount {
        fee_amount.to_u256_from_eth().map_err(err_from!())?
    } else {
        return Err(err_custom_create!(
            "Fee amount not specified. Use --fee-amount"
        ));
    };

    if !opt.skip_balance_check {
        let block_info = get_latest_block_info(web3.clone()).await?;
        let balance_result = get_token_balance(
            web3.clone(),
            GetBalanceArgs {
                address: from,
                token_address: Some(glm_address),
                call_with_details: None,
                block_number: Some(block_info.block_number),
                chain_id: Some(chain_id),
            },
        )
        .await?;

        let token_balance = balance_result.token_balance.ok_or(err_custom_create!(
            "Token balance not found for account {:#x}",
            from
        ))?;
        if token_balance < amount + fee_amount {
            return Err(err_custom_create!(
                "You don't have enough: {} GLM on network with chain id: {} and account {:#x}",
                token_balance,
                chain_id,
                from
            ));
        };
    }

    let mut db_transaction = conn.begin().await.map_err(err_from!())?;
    let filter = "method=\"LOCK.deposit\" AND fee_paid is NULL";
    let tx_existing = get_transactions(
        &mut *db_transaction,
        Some(from),
        Some(filter),
        None,
        None,
        Some(chain_id as i64),
    )
    .await
    .map_err(err_from!())?;

    if let Some(tx) = tx_existing.first() {
        return Err(err_custom_create!(
            "You already have a pending deposit transaction with id: {}",
            tx.id
        ));
    }

    let deposit_tx = create_create_deposit(
        from,
        opt.lock_contract_address,
        chain_id,
        None,
        CreateDepositArgs {
            deposit_nonce: opt.deposit_nonce,
            deposit_spender: opt.spender,
            deposit_amount: amount,
            deposit_fee_amount: fee_amount,
            deposit_timestamp: opt.timestamp,
        },
    )?;

    let deposit_tx = insert_tx(&mut *db_transaction, &deposit_tx)
        .await
        .map_err(err_from!())?;
    db_transaction.commit().await.map_err(err_from!())?;

    log::info!("Create deposit added to queue: {}", deposit_tx.id);
    Ok(())
}

pub async fn get_token_balance(
    web3: Arc<Web3RpcPool>,
    args: GetBalanceArgs,
) -> Result<GetBalanceResult, PaymentError> {
    crate::eth::get_balance(web3, args).await
}

pub async fn get_unpaid_token_amount(
    conn: &SqlitePool,
    chain_id: i64,
    token_address: Address,
    sender: Address,
) -> Result<U256, PaymentError> {
    let transfers = get_unpaid_token_transfers(conn, chain_id, sender)
        .await
        .map_err(err_from!())?;
    let mut sum = U256::default();
    for transfer in transfers {
        if let Some(token_addr) = transfer.token_addr {
            let token_addr = Address::from_str(&token_addr).map_err(err_from!())?;
            if token_addr != token_address {
                return Err(err_custom_create!(
                    "Token address mismatch table token_transfer: {} != {}, id: {}",
                    transfer.id,
                    token_addr,
                    token_address
                ));
            }
            sum += transfer.token_amount.to_u256().map_err(err_from!())?
        }
    }
    Ok(sum)
}

// This is for now very limited check. It needs lot more work to be complete
pub async fn verify_transaction(
    web3: Arc<Web3RpcPool>,
    chain_id: i64,
    tx_hash: H256,
    sender: Address,
    receiver: Address,
    amount: U256,
    glm_address: Address,
) -> Result<VerifyTransactionResult, PaymentError> {
    let (chain_tx_dao, transfers) =
        match find_receipt_extended(web3, tx_hash, chain_id, glm_address).await? {
            FindReceiptParseResult::Success((chain_tx_dao, transfers)) => (chain_tx_dao, transfers),
            FindReceiptParseResult::Failure(str) => {
                return Ok(VerifyTransactionResult::Rejected(format!(
                    "Transaction cannot be parsed {str}"
                )))
            }
        };

    if chain_tx_dao.chain_status == 1 {
        //one transaction can contain multiple transfers. Search for ours.
        for transfer in transfers {
            log::info!(
                "Verifying {tx_hash:#x}: Found transfers on chain: {:?}",
                transfer
            );
            if Address::from_str(&transfer.receiver_addr).map_err(err_from!())? == receiver
                && (Address::from_str(&transfer.from_addr).map_err(err_from!())? == sender
                    || Address::from_str(&chain_tx_dao.from_addr).map_err(err_from!())? == sender)
            {
                let tx_amount = U256::from_dec_str(&transfer.token_amount).map_err(err_from!())?;
                return if tx_amount >= amount {
                    log::info!("Transaction found and verified: {}", tx_hash);
                    Ok(VerifyTransactionResult::Verified { amount: tx_amount })
                } else {
                    log::warn!(
                        "Transaction found but amount insufficient: {}: {}/{}",
                        tx_hash,
                        transfer.token_amount,
                        amount
                    );
                    Ok(VerifyTransactionResult::Rejected(
                        "Transaction found but amount insufficient".to_string(),
                    ))
                };
            }
        }
        log::warn!("Transaction found but not matching: {}", tx_hash);
        Ok(VerifyTransactionResult::Rejected(
            "Transaction found but not matching".to_string(),
        ))
    } else {
        Ok(VerifyTransactionResult::Rejected(
            "Transaction not found".to_string(),
        ))
    }
}

pub async fn remove_transaction_force(
    conn: &SqlitePool,
    tx_id: i64,
) -> Result<Option<Vec<i64>>, PaymentError> {
    let mut db_transaction = conn
        .begin()
        .await
        .map_err(|err| err_custom_create!("Error beginning transaction {err}"))?;

    match get_transaction_chain(&mut db_transaction, tx_id).await {
        Ok(txs) => {
            for tx in &txs {
                //if tx is allowance then remove all references to it
                cleanup_allowance_tx(&mut *db_transaction, tx.id)
                    .await
                    .map_err(err_from!())?;
                //if tx is token_transfer then remove all references to it
                cleanup_token_transfer_tx(&mut *db_transaction, tx.id)
                    .await
                    .map_err(err_from!())?;
                delete_tx(&mut *db_transaction, tx.id)
                    .await
                    .map_err(err_from!())?;
            }
            db_transaction.commit().await.map_err(err_from!())?;
            Ok(Some(txs.iter().map(|tx| tx.id).collect()))
        }
        Err(e) => {
            log::error!("Error getting transaction: {}", e);
            Err(err_custom_create!("Error getting transaction: {}", e))
        }
    }
}

pub async fn remove_last_unsent_transactions(
    conn: SqlitePool,
) -> Result<Option<i64>, PaymentError> {
    let mut db_transaction = conn
        .begin()
        .await
        .map_err(|err| err_custom_create!("Error beginning transaction {err}"))?;
    match get_last_unsent_tx(&mut *db_transaction, 0).await {
        Ok(tx) => {
            if let Some(tx) = tx {
                cleanup_token_transfer_tx(&mut *db_transaction, tx.id)
                    .await
                    .map_err(err_from!())?;
                delete_tx(&mut *db_transaction, tx.id)
                    .await
                    .map_err(err_from!())?;
                db_transaction.commit().await.map_err(err_from!())?;
                Ok(Some(tx.id))
            } else {
                Ok(None)
            }
        }
        Err(e) => {
            log::error!("Error getting last unsent transaction: {}", e);
            Err(err_custom_create!(
                "Error getting last unsent transaction: {}",
                e
            ))
        }
    }
}
pub async fn send_driver_event(
    event_sender: &Option<mpsc::Sender<DriverEvent>>,
    event: DriverEventContent,
) {
    if let Some(event_sender) = event_sender {
        let event = DriverEvent::now(event);
        if let Err(e) = event_sender.send(event).await {
            log::error!("Error sending event: {}", e);
        }
    }
}
