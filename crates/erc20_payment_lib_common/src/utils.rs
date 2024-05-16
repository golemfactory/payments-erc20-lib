use chrono::{DateTime, Utc};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use std::env;
use std::error::Error;
use std::fmt::{Display, Formatter};
use web3::types::U256;

pub fn datetime_from_u256_timestamp(timestamp: U256) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(timestamp.as_u64() as i64, 0)
}

pub fn datetime_from_u256_with_option(timestamp: U256) -> Option<DateTime<Utc>> {
    if timestamp.is_zero() {
        None
    } else {
        datetime_from_u256_timestamp(timestamp)
    }
}

pub fn get_env_bool_value(env_name: &str) -> bool {
    env::var(env_name)
        .map(|v| {
            if v == "1" || v == "true" {
                true
            } else {
                if v != "0" && v != "false" {
                    log::warn!("Invalid value for {}: {} assuming false", env_name, v);
                }
                false
            }
        })
        .unwrap_or(false)
}

#[derive(Debug, Clone)]
pub struct ConversionError {
    pub msg: String,
}

impl ConversionError {
    pub fn from(msg: String) -> Self {
        Self { msg }
    }
}

impl Display for ConversionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Error during conversion: {}", self.msg)
    }
}

impl Error for ConversionError {
    fn description(&self) -> &str {
        "Conversion error"
    }
}

fn compute_base(num_decimals: u32) -> rust_decimal::Decimal {
    if num_decimals == 18 {
        Decimal::new(1000000000000000000, 0)
    } else if num_decimals == 6 {
        Decimal::new(1000000, 0)
    } else {
        Decimal::from(10_u128.pow(num_decimals))
    }
}

///good from one gwei up to at least one billion ethers
fn rust_dec_to_u256_strict(
    dec_amount: rust_decimal::Decimal,
    decimals: Option<u32>,
) -> Result<U256, ConversionError> {
    let num_decimals = decimals.unwrap_or(18);
    if num_decimals > 18 {
        return Err(ConversionError {
            msg: format!("Decimals: {num_decimals} cannot be greater than 18"),
        });
    }

    let dec_base = compute_base(num_decimals);
    //println!("dec: {}, number scale: {}", dec_base, dec_base.scale());

    let dec_mul = dec_amount.checked_mul(dec_base).ok_or(ConversionError {
        msg: "Overflow during conversion".to_string(),
    })?;
    //println!("number: {}, number scale: {}", dec_mul, dec_mul.scale());

    let dec_mul = dec_mul.normalize();
    //println!("number normalized: {}", dec_mul);

    if dec_mul.fract() != Decimal::from(0) {
        return Err(ConversionError::from(format!(
            "Number cannot have a fractional part {dec_mul}"
        )));
    }
    let u128 = dec_mul.to_u128().ok_or_else(|| {
        ConversionError::from(format!("Number cannot be converted to u128 {dec_mul}"))
    })?;
    Ok(U256::from(u128))
}

#[derive(Debug, Clone, Copy)]
pub enum Decimals {
    Zero = 0,
    Six = 6,
    Nine = 9,
    Eighteen = 18,
}

#[allow(dead_code)]
fn rust_dec_to_u256(dec_amount: rust_decimal::Decimal, decimals: Decimals) -> U256 {
    if dec_amount < Decimal::from(0) {
        return U256::zero();
    }
    let num_decimals = match decimals {
        Decimals::Zero => 0,
        Decimals::Six => 6,
        Decimals::Nine => 9,
        Decimals::Eighteen => 18,
    };

    let dec_base = compute_base(num_decimals);
    //println!("dec: {}, number scale: {}", dec_base, dec_base.scale());

    let dec_mul = match dec_amount.checked_mul(dec_base) {
        Some(dec_mul) => dec_mul,
        None => {
            log::warn!(
                "Overflow during multiplication dec_amount: {} dec_base: {}. Using saturated mul",
                dec_amount,
                dec_base
            );
            dec_amount.saturating_mul(dec_base)
        }
    };

    //println!("number: {}, number scale: {}", dec_mul, dec_mul.scale());

    let dec_mul = dec_mul.normalize();
    //println!("number normalized: {}", dec_mul);

    if dec_mul.fract() != Decimal::from(0) {
        log::warn!("Number have a fractional part which will be truncated {dec_mul}");
    }
    let u128 = dec_mul.to_u128().unwrap_or(
        // to_u128 is failing only if dec_mul is negative which is not possible here
        0,
    );

    U256::from(u128)
}

fn u256_to_rust_dec(
    amount: U256,
    decimals: Option<u32>,
) -> Result<rust_decimal::Decimal, ConversionError> {
    let num_decimals = decimals.unwrap_or(18);
    if num_decimals > 18 {
        return Err(ConversionError {
            msg: format!("Decimals: {num_decimals} cannot be greater than 18"),
        });
    }

    let dec_base = compute_base(num_decimals);

    //max value supported by rust_decimal
    if amount >= U256::from(79228162514264337593543950336_u128) {
        return Err(ConversionError {
            msg: format!(
                "Amount greater than max rust_decimal: {amount}>=79228162514264337593543950336"
            ),
        });
    }

    Ok(Decimal::from(amount.as_u128()) / dec_base)
}

fn u256_to_gwei(amount: U256) -> Result<Decimal, ConversionError> {
    u256_to_rust_dec(amount, Some(9))
}

pub trait U256ConvExt {
    fn to_gwei(&self) -> Result<Decimal, ConversionError>;
    fn to_gwei_saturate(&self) -> Decimal;
    fn to_eth(&self) -> Result<Decimal, ConversionError>;
    fn to_eth_saturate(&self) -> Decimal;
    fn to_gwei_str(&self) -> String;
    fn to_eth_str(&self) -> String;
    fn to_gwei_str_with_precision(&self, precision: u8) -> String;
    fn to_eth_str_with_precision(&self, precision: u8) -> String;
}

impl U256ConvExt for U256 {
    fn to_gwei(&self) -> Result<Decimal, ConversionError> {
        u256_to_gwei(*self)
    }
    fn to_gwei_saturate(&self) -> Decimal {
        u256_to_gwei(*self).unwrap_or(Decimal::from(10000000000000_u64))
    }
    fn to_eth(&self) -> Result<Decimal, ConversionError> {
        u256_to_eth(*self)
    }
    fn to_eth_saturate(&self) -> Decimal {
        u256_to_eth(*self).unwrap_or(Decimal::from(10000000000_u64))
    }
    fn to_gwei_str(&self) -> String {
        u256_to_decimal_string(*self, Decimals::Nine, None)
    }
    fn to_eth_str(&self) -> String {
        u256_to_decimal_string(*self, Decimals::Eighteen, None)
    }
    fn to_gwei_str_with_precision(&self, precision: u8) -> String {
        u256_to_decimal_string(*self, Decimals::Nine, Some(precision as usize))
    }
    fn to_eth_str_with_precision(&self, precision: u8) -> String {
        u256_to_decimal_string(*self, Decimals::Eighteen, Some(precision as usize))
    }
}

pub trait StringConvExt {
    fn to_gwei(&self) -> Result<Decimal, ConversionError>;
    fn to_eth(&self) -> Result<Decimal, ConversionError>;
    fn to_u256(&self) -> Result<U256, ConversionError>;
}
impl StringConvExt for String {
    fn to_gwei(&self) -> Result<Decimal, ConversionError> {
        self.to_u256()?.to_gwei()
    }
    fn to_eth(&self) -> Result<Decimal, ConversionError> {
        self.to_u256()?.to_eth()
    }

    fn to_u256(&self) -> Result<U256, ConversionError> {
        U256::from_dec_str(self).map_err(|err| {
            ConversionError::from(format!("Invalid string when converting: {err:?}"))
        })
    }
}

pub trait DecimalConvExt {
    fn to_u256_from_gwei(&self) -> Result<U256, ConversionError>;
    fn to_u256_from_eth(&self) -> Result<U256, ConversionError>;
}

impl DecimalConvExt for Decimal {
    fn to_u256_from_gwei(&self) -> Result<U256, ConversionError> {
        rust_dec_to_u256_strict(*self, Some(9))
    }
    fn to_u256_from_eth(&self) -> Result<U256, ConversionError> {
        rust_dec_to_u256_strict(*self, Some(18))
    }
}

fn u256_to_eth(amount: U256) -> Result<Decimal, ConversionError> {
    u256_to_rust_dec(amount, Some(18))
}

pub fn u256_eth_from_str(val: &str) -> Result<(U256, Decimal), ConversionError> {
    let u256 = U256::from_dec_str(val)
        .map_err(|err| ConversionError::from(format!("Invalid string when converting: {err:?}")))?;
    let eth = u256_to_eth(u256)?;
    Ok((u256, eth))
}

pub fn u256_gwei_from_str(val: &str) -> Result<(U256, Decimal), ConversionError> {
    let u256 = U256::from_dec_str(val)
        .map_err(|err| ConversionError::from(format!("Invalid string when converting: {err:?}")))?;
    let gwei = u256_to_gwei(u256)?;
    Ok((u256, gwei))
}

/// precision cannot be greater than decimals (it is capped automatically)
pub fn u256_to_decimal_string(
    amount: U256,
    decimals: Decimals,
    precision: Option<usize>,
) -> String {
    let str = &amount.to_string();
    let mut str_rev: Vec<char> = str.chars().rev().collect();
    let precision = precision.map(|p| std::cmp::min(p, decimals as usize));

    #[allow(clippy::same_item_push)]
    for _ in 0..(decimals as usize) {
        str_rev.push('0');
    }

    str_rev.insert(decimals as usize, '.');

    let str: String = str_rev.iter().rev().collect();
    let str = str.trim_matches('0').to_string();
    let mut str = if str.starts_with('.') {
        "0".to_string() + &str
    } else {
        str
    };

    let idx_of_dot = str.find('.').unwrap_or(str.len()) as i64;
    let number_of_digit_at_right = str.len() as i64 - idx_of_dot - 1;

    if let Some(precision) = precision {
        let add_zeroes = precision as i64 - number_of_digit_at_right;
        if add_zeroes > 0 {
            for _ in 0..add_zeroes {
                str.push('0');
            }
        } else {
            for _ in 0..(-add_zeroes) {
                str.pop();
            }
        }
    }

    str = str.trim_end_matches('.').to_string();

    str
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;
    use std::str::FromStr;

    #[test]
    #[rustfmt::skip]
    fn test_rust_u256_to_str() {
        assert_eq!(u256_to_decimal_string(U256::from(0), Decimals::Zero, None), "0");
        assert_eq!(u256_to_decimal_string(U256::from(0), Decimals::Six, None), "0");
        assert_eq!(u256_to_decimal_string(U256::from(0), Decimals::Nine, None), "0");
        assert_eq!(u256_to_decimal_string(U256::from(0), Decimals::Eighteen, None), "0");
        assert_eq!(u256_to_decimal_string(U256::from(1), Decimals::Zero, None), "1");
        assert_eq!(u256_to_decimal_string(U256::from(1), Decimals::Six, None), "0.000001");
        assert_eq!(u256_to_decimal_string(U256::from(1), Decimals::Nine, None), "0.000000001");
        assert_eq!(u256_to_decimal_string(U256::from(1), Decimals::Eighteen, None), "0.000000000000000001");
        assert_eq!(u256_to_decimal_string(U256::from(1), Decimals::Six, Some(0)), "0");
        assert_eq!(u256_to_decimal_string(U256::from(1), Decimals::Six, Some(3)), "0.000");
        assert_eq!(u256_to_decimal_string(U256::from(1), Decimals::Six, Some(6)), "0.000001");
        assert_eq!(u256_to_decimal_string(U256::from(1), Decimals::Six, Some(9)), "0.000001");

        let max_u256_str = "115792089237316195423570985008687907853269984665640564039457584007913129639935";
        assert_eq!(u256_to_decimal_string(U256::from_dec_str(max_u256_str).unwrap(), Decimals::Zero, Some(2)),
                   "115792089237316195423570985008687907853269984665640564039457584007913129639935");
        assert_eq!(u256_to_decimal_string(U256::from_dec_str(max_u256_str).unwrap(), Decimals::Eighteen, Some(2)),
            "115792089237316195423570985008687907853269984665640564039457.58");

        let mut rng = rand::thread_rng();
        for _ in 0..1000 {
            let rand: u64 = rng.gen();

            let u256 = U256::from(rand) / U256::from(1000000_u64);
            assert_eq!(u256.to_string(), u256_to_decimal_string(U256::from(rand), Decimals::Six, Some(0)));
        }

        assert_eq!(u256_to_decimal_string(U256::from(1000000000000000000_u128), Decimals::Eighteen, None), "1");
        assert_eq!(u256_to_decimal_string(U256::from(1000000000000000000000000000000000000_u128), Decimals::Eighteen, None), "1000000000000000000");
        assert_eq!(u256_to_decimal_string(U256::from(1000000000000000000000000000000000000_u128), Decimals::Eighteen, Some(5)), "1000000000000000000.00000");
        assert_eq!(u256_to_decimal_string(U256::from(1000000000000000000660000000000000000_u128), Decimals::Eighteen, None), "1000000000000000000.66");
        assert_eq!(u256_to_decimal_string(U256::from(1000000000000000000660000000000000778_u128), Decimals::Eighteen, None), "1000000000000000000.660000000000000778");
        assert_eq!(u256_to_decimal_string(U256::from(1000000000000000000660000000000000778_u128), Decimals::Eighteen, Some(16)), "1000000000000000000.6600000000000007");
    }

    #[test]
    fn test_rust_decimal_conversion() {
        let dec_gwei = Decimal::new(1, 18);
        let res = rust_dec_to_u256_strict(dec_gwei, None).unwrap();
        assert_eq!(res, U256::from(1));

        let res = rust_dec_to_u256_strict(dec_gwei / Decimal::from(2), None);
        println!("res: {res:?}");
        assert!(res.err().unwrap().msg.contains("fractional"));

        let res = rust_dec_to_u256_strict(dec_gwei / Decimal::from(2), Some(19));
        println!("res: {res:?}");
        assert!(res.err().unwrap().msg.contains("greater than 18"));

        let res = rust_dec_to_u256_strict(Decimal::from(8777666555_u64), None).unwrap();
        println!("res: {res:?}");
        assert_eq!(
            res,
            U256::from(8777666555_u64) * U256::from(1000000000000000000_u64)
        );

        let res = rust_dec_to_u256_strict(Decimal::from(8777666555_u64) + dec_gwei, None).unwrap();
        println!("res: {res:?}");
        assert_eq!(res, U256::from(8777666555000000000000000001_u128));

        let res = rust_dec_to_u256_strict(Decimal::from(0), None).unwrap();
        println!("res: {res:?}");
        assert_eq!(res, U256::from(0));

        let res = rust_dec_to_u256_strict(Decimal::from(1), Some(0)).unwrap();
        println!("res: {res:?}");
        assert_eq!(res, U256::from(1));

        let res = rust_dec_to_u256_strict(Decimal::from(1), Some(6)).unwrap();
        println!("res: {res:?}");
        assert_eq!(res, U256::from(1000000));

        let res = rust_dec_to_u256_strict(Decimal::from(1), Some(9)).unwrap();
        println!("res: {res:?}");
        assert_eq!(res, U256::from(1000000000));

        let res =
            rust_dec_to_u256_strict(Decimal::from_str("123456789.123456789").unwrap(), Some(18))
                .unwrap();
        println!("res: {res:?}");
        assert_eq!(
            res,
            U256::from_dec_str("123456789123456789000000000").unwrap()
        );

        //this should result in overflow, because 79228162514264337593543950336 == 2**96
        let res = rust_dec_to_u256_strict(
            Decimal::from_str("79228162514.264337593543950336").unwrap(),
            Some(18),
        );
        println!("res: {res:?}");
        assert!(res.err().unwrap().msg.to_lowercase().contains("overflow"));

        //this is the max value that can be represented by rust decimal
        let res = rust_dec_to_u256_strict(
            Decimal::from_str("79228162514.264337593543950335").unwrap(),
            Some(18),
        )
        .unwrap();
        println!("res: {res:?}");
        assert_eq!(res, U256::from(79228162514264337593543950335_u128));

        //this is the max value that can be represented by rust decimal
        let res = rust_dec_to_u256_strict(
            Decimal::from_str("79228162514264337593543950335").unwrap(),
            Some(0),
        )
        .unwrap();
        println!("res: {res:?}");
        assert_eq!(res, U256::from(79228162514264337593543950335_u128));

        //this is the max value that can be represented by rust decimal
        let res = rust_dec_to_u256_strict(
            Decimal::from_str("79228162514264337593543.950335").unwrap(),
            Some(6),
        )
        .unwrap();
        println!("res: {res:?}");
        assert_eq!(res, U256::from(79228162514264337593543950335_u128));

        //this is the max value that can be represented by rust decimal
        let res = rust_dec_to_u256_strict(
            Decimal::from_str("792281625142643.37593543950335").unwrap(),
            Some(14),
        )
        .unwrap();
        println!("res: {res:?}");
        assert_eq!(res, U256::from(79228162514264337593543950335_u128));
        //assert_eq!(res, U256::zero());

        let res = rust_dec_to_u256(
            Decimal::from_str("79228162514.264337593543950335").unwrap(),
            Decimals::Eighteen,
        );
        assert_eq!(res, U256::from(79228162514264337593543950335_u128));

        let res = rust_dec_to_u256(
            Decimal::from_str("2514.26433759354395033559999").unwrap(),
            Decimals::Eighteen,
        );
        assert_eq!(res, U256::from(2514264337593543950335_u128));
    }
}
