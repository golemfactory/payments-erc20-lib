use chrono::{DateTime, Utc};
use erc20_payment_lib_common::err_custom_create;
use erc20_payment_lib_common::error::PaymentError;
use serde::Serialize;
use std::fmt::{Debug, Display, Formatter};
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use super::Signer;
use web3::types::{Address, SignedTransaction, TransactionParameters, H160};

#[derive(Clone, Serialize)]
pub struct SignerAccount {
    pub address: Address,
    #[serde(skip)]
    pub signer: Arc<Box<dyn Signer + Send + Sync>>,
    pub(crate) external_gather_time: Arc<Mutex<Option<DateTime<Utc>>>>,
    #[serde(skip)]
    pub(crate) jh: Arc<Mutex<Vec<Option<JoinHandle<()>>>>>,
}

impl Debug for SignerAccount {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "SignerAccount {{ address: {:#x} }}", self.address)
    }
}

impl Display for SignerAccount {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#x}", self.address)
    }
}

impl SignerAccount {
    pub fn new(address: H160, signer: Arc<Box<dyn Signer + Send + Sync>>) -> Self {
        Self {
            address,
            signer,
            external_gather_time: Arc::new(Mutex::new(None)),
            jh: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn is_active(&self) -> bool {
        let jh_guard = self.jh.lock().unwrap();
        for jh in jh_guard.iter() {
            if let Some(jh) = (*jh).as_ref() {
                if !jh.is_finished() {
                    return true;
                }
            }
        }
        false
    }

    pub async fn check_if_sign_possible(&self) -> Result<(), PaymentError> {
        match timeout(
            std::time::Duration::from_secs(5),
            self.signer.check_if_sign_possible(self.address),
        )
        .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(err)) => Err(err_custom_create!("Sign returned error {err:?}")),
            Err(err) => Err(err_custom_create!("Sign check timed out {err:?}")),
        }
    }

    pub async fn sign(&self, tp: TransactionParameters) -> Result<SignedTransaction, PaymentError> {
        match timeout(
            std::time::Duration::from_secs(5),
            self.signer.sign(self.address, tp),
        )
        .await
        {
            Ok(Ok(signed)) => Ok(signed),
            Ok(Err(err)) => Err(err_custom_create!("Sign returned error {err:?}")),
            Err(err) => Err(err_custom_create!("Sign check timed out {err:?}")),
        }
    }
}
