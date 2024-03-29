use erc20_payment_lib::config::AdditionalOptions;
use erc20_payment_lib::misc::load_private_keys;
use erc20_payment_lib::runtime::{PaymentRuntime, PaymentRuntimeArgs};
use erc20_payment_lib::signer::PrivateKeySigner;
use erc20_payment_lib::transaction::create_token_transfer;
use erc20_payment_lib_common::ops::insert_token_transfer;
use erc20_payment_lib_common::utils::U256ConvExt;
use erc20_payment_lib_common::DriverEvent;
use erc20_payment_lib_common::DriverEventContent::*;
use erc20_payment_lib_test::*;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use web3::types::{Address, U256};

#[tokio::test(flavor = "multi_thread")]
#[rustfmt::skip]
async fn test_wrong_chain_id() -> Result<(), anyhow::Error> {
    // *** TEST SETUP ***

    let geth_container = exclusive_geth_init(Duration::from_secs(30)).await;
    let conn = setup_random_memory_sqlite_conn().await;

    let proxy_url_base = format!("http://127.0.0.1:{}", geth_container.web3_proxy_port);
    let proxy_key = "erc20_transfer";
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<DriverEvent>(1);
    let receiver_loop = tokio::spawn(async move {
        let mut transfer_finished_message_count = 0;
        let mut tx_confirmed_message_count = 0;
        let tx_invalid_chain_id_message_count = 0;
        let mut fee_paid = U256::from(0_u128);
        while let Some(msg) = receiver.recv().await {
            log::info!("Received message: {:?}", msg);

            match msg.content {
                TransferFinished(transfer_finished) => {
                    transfer_finished_message_count += 1;
                    fee_paid += U256::from_dec_str(&transfer_finished.token_transfer_dao.fee_paid.expect("fee paid should be set")).expect("fee paid should be a valid U256");
                }
                TransactionStuck(reason) => {
                    //todo - dont ignore it check if proper status
                    log::info!("Transaction stuck: {:?}", reason);
                },
                TransactionFailed(reason) => {
                    #[allow(clippy::match_single_binding)]
                    match reason {
                        _ => {
                            log::error!("Unexpected transaction failed reason: {:?}", reason);
                            panic!("Unexpected transaction failed reason: {:?}", reason)
                        }
                    }
                },
                TransactionConfirmed(_tx_dao) => {
                    tx_confirmed_message_count += 1;
                },
                Web3RpcMessage(_) => { },
                StatusChanged(_) => { },
                _ => {
                    //maybe remove this if caused too much hassle to maintain
                    panic!("Unexpected message: {:?}", msg);
                }
            }
        }

        assert!(tx_invalid_chain_id_message_count == 0);
        assert_eq!(tx_confirmed_message_count, 0);
        assert_eq!(transfer_finished_message_count, 0);
        fee_paid
    });

    {
        let config = create_default_config_setup(&proxy_url_base, proxy_key).await;

        //load private key for account 0x653b48E1348F480149047AA3a58536eb0dbBB2E2
        let private_keys = load_private_keys("c2b876dd5ef1bcab6864249c58dfea6018538d67d0237f105ff8b54d32fb98e1")?;
        let signer = PrivateKeySigner::new(private_keys.0.clone());

        //add single gas transaction to database
        insert_token_transfer(
            &conn,
            &create_token_transfer(
                Address::from_str("0x653b48E1348F480149047AA3a58536eb0dbBB2E2").unwrap(),
                Address::from_str("0x41162E565ebBF1A52eC904c7365E239c40d82568").unwrap(),
                3234,
                Some("test_payment"),
                None,
                U256::from(456000000000000222_u128),
                None,
            )
        ).await?;

        // *** TEST RUN ***

        let sp = PaymentRuntime::new(
            PaymentRuntimeArgs {
                secret_keys: private_keys.0,
                db_filename: Default::default(),
                config: config.clone(),
                conn: Some(conn.clone()),
                options: Some(AdditionalOptions {
                    keep_running: false,
                    ..Default::default()
                }),
                broadcast_sender: None,
                mspc_sender: Some(sender),
                extra_testing: None,
            },
            Arc::new(Box::new(signer)),
        ).await?;
        //exit after some time

        tokio::time::sleep(Duration::from_secs(10)).await;
        sp.abort_tasks();
    }

    {
        // *** RESULT CHECK ***
        let fee_paid = receiver_loop.await.unwrap();
        assert_eq!(fee_paid, U256::zero());
        log::info!("fee paid: {}", fee_paid.to_eth().unwrap());


        let res = test_get_balance(&proxy_url_base, "0x653b48E1348F480149047AA3a58536eb0dbBB2E2,0x41162E565ebBF1A52eC904c7365E239c40d82568").await?;
        assert_eq!(res["0x41162e565ebbf1a52ec904c7365e239c40d82568"].gas_decimal,   Some("0".to_string()));
        assert_eq!(res["0x41162e565ebbf1a52ec904c7365e239c40d82568"].token_decimal, Some("0".to_string()));
    }

    Ok(())
}
