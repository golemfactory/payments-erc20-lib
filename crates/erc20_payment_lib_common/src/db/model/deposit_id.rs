use crate::err_custom_create;
use crate::error::PaymentError;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use web3::types::{Address, U256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DepositId {
    pub deposit_id: U256,
    pub lock_address: Address,
}

impl DepositId {
    pub fn funder(&self) -> Address {
        let bytes: [u8; 32] = self.deposit_id.into();
        Address::from_slice(&bytes[0..20])
    }

    pub fn nonce(&self) -> u64 {
        (self.deposit_id & U256::from(0xffffffffffffffffu64)).as_u64()
    }

    pub fn to_db_string(&self) -> String {
        format!("{:#x}-{:#x}", self.deposit_id, self.lock_address)
    }

    pub fn from_db_string(s: &str) -> Result<Self, PaymentError> {
        let parts: Vec<&str> = s.split('-').collect();

        if parts.len() != 2 {
            return Err(err_custom_create!("Invalid depositId format"));
        }
        let deposit_id = U256::from_str_radix(parts[0], 16)
            .map_err(|e| err_custom_create!("Invalid depositId: {}", e))?;
        let lock_address = Address::from_str(parts[1])
            .map_err(|e| err_custom_create!("Invalid lockAddress: {}", e))?;
        Ok(DepositId {
            deposit_id,
            lock_address,
        })
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "depositId": format!("{:#x}", self.deposit_id),
            "lockAddress": format!("{:#x}", self.lock_address),
            "funder": format!("{:#x}", self.funder()),
            "nonce": self.nonce(),
        })
    }
}
