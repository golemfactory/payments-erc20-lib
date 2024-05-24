use crate::config_setup::create_default_config_setup;
use erc20_payment_lib_common::error::PaymentError;
use erc20_payment_lib_extra::{account_balance, BalanceOptions, BalanceResult};
use std::collections::BTreeMap;

pub async fn test_get_balance(
    proxy_url_base: &str,
    accounts: &str,
) -> Result<BTreeMap<String, BalanceResult>, PaymentError> {
    let config_check = create_default_config_setup(proxy_url_base, "check").await;
    let account_balance_options = BalanceOptions {
        chain_name: "dev".to_string(),
        accounts: Some(accounts.to_string()),
        hide_gas: false,
        hide_token: false,
        block_number: None,
        tasks: 4,
        interval: Some(0.001),
        no_wrapper_contract: false,
    };

    account_balance(account_balance_options.clone(), &config_check).await
}
