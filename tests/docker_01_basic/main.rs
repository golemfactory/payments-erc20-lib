use erc20_payment_lib_test::test_durability;

mod single_erc20_transfer;
mod single_gas_transfer;

#[actix_rt::test]
async fn test_durability_1() -> Result<(), anyhow::Error> {
    test_durability(1, 0.1, 10).await
}

#[actix_rt::test]
async fn test_durability_20() -> Result<(), anyhow::Error> {
    test_durability(20, 0.1, 10).await
}
