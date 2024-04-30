use rustc_hex::FromHexError;
use std::str::FromStr;
use web3::types::Address;

pub mod attestation;
pub mod check_rpc;
pub mod deposit;
pub mod scan_chain;

pub fn check_address_name(n: &str) -> Result<Address, FromHexError> {
    match n {
        "funds" => Address::from_str("0x333dFEa0C940Dc9971C32C69837aBE14207F9097"),
        "dead" => Address::from_str("0x000000000000000000000000000000000000dEaD"),
        "null" => Address::from_str("0x0000000000000000000000000000000000000000"),
        "random" => Ok(Address::from(rand::Rng::gen::<[u8; 20]>(
            &mut rand::thread_rng(),
        ))),
        _ => Address::from_str(n),
    }
}
