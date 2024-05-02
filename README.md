# rust_erc20_payment

# Notes

* all addresses/txs in database are in hex/lowercase format. 

# Frontend

Link to frontend repo:
https://github.com/scx1332/erc20_driver_frontend

# Sample environment setup

ETH_PRIVATE_KEY=0000000000000000000000000000000000000000000000000000000000000000
PROVIDER_URL=https://rpc-mumbai.matic.today
RUST_LOG=debug,sqlx::query=info,web=warn

# Sample runs

```
cargo run -- transfer --plain-eth --amounts=1,2,3,4 --receivers=0xA000000000000000000000000000000000050001,0xA000000000000000000000000000000000050002,0xa000000000000000000000000000000000050003,0xa000000000000000000000000000000000050004
cargo run -- transfer --token-addr=0x2036807b0b3aaf5b1858ee822d0e111fddac7018 --amounts=1,2,3,4 --receivers=0xA000000000000000000000000000000000050001,0xA000000000000000000000000000000000050002,0xa000000000000000000000000000000000050003,0xa000000000000000000000000000000000050004
cargo run --example generate_transfers -- --chain-name dev --address-pool-size 10000 --amounts-pool-size 10000 --generate-count 100
```

prepare test transfers into db, it generates 100 random GLM transfers to 10 unique addresses

```cargo run --example generate_transfers -- --generate-count 100 --address-pool-size 10 --amounts-pool-size=100```

dry run without processing transactions

```cargo run -- process --generate-tx-only=1```

Useful command to see transactions being processed
```sql
SELECT id,
       (CAST((julianday(broadcast_date) - 2440587.5)*86400000 AS INTEGER) - CAST((julianday(created_date) - 2440587.5)*86400000 AS INTEGER)) / 1000.0 as broadcast_delay,
       broadcast_count,
       (CAST((julianday(confirm_date) - 2440587.5)*86400000 AS INTEGER) - CAST((julianday(broadcast_date) - 2440587.5)*86400000 AS INTEGER)) / 1000.0 as confirm_delay,
       tx_hash,
       *
FROM tx
order by created_date desc
```

Clean all transactions and transfers
```sql
DELETE FROM token_transfer;
DELETE FROM tx;
```

# TODO

- [x] Add error handling in gather_transactions, now SQL will loop forever, when hit error in gather

# Example get balance

Get balance running 4 tasks in parallel
```
cargo run -- account-balance --tasks 4 -c polygon -a 0x75be52afd54a13b6c98490b4db495aa79b609d58,0x7caac644722316101807e0d55f838f7851a97031,0x52a258ed593c793251a89bfd36cae158ee9fc4f8,0x04e2dc96afecdf72221882e1cee039cab4d443e0,0xa32a0edc623d86e623f58e7c4174023a80a67ddf,0x7cb53b925a79fb15c348fcfd9abcf2287854d33a,0x8cf88c473b6cb40b8d37cdd93e6c8118c14a6e60,0xa96d3f3e177687fb0b5f990d5c4000923b49430b,0x92fb36230b50a87a39ba3237c996caf5a39b230b,0x0c4d7a995aa9846ef25e1a347a8711c8b534b5a6,0x698076ae39e7e44bcd2bbe15f0486c8d44bb4e6f
```
Be nicer to endpoint one by one
```
cargo run -- account-balance --tasks 1 -c polygon -a 0x75be52afd54a13b6c98490b4db495aa79b609d58,0x7caac644722316101807e0d55f838f7851a97031,0x52a258ed593c793251a89bfd36cae158ee9fc4f8,0x04e2dc96afecdf72221882e1cee039cab4d443e0,0xa32a0edc623d86e623f58e7c4174023a80a67ddf,0x7cb53b925a79fb15c348fcfd9abcf2287854d33a,0x8cf88c473b6cb40b8d37cdd93e6c8118c14a6e60,0xa96d3f3e177687fb0b5f990d5c4000923b49430b,0x92fb36230b50a87a39ba3237c996caf5a39b230b,0x0c4d7a995aa9846ef25e1a347a8711c8b534b5a6,0x698076ae39e7e44bcd2bbe15f0486c8d44bb4e6f
```
Be nicer to endpoint rate limit every 2 seconds
```
cargo run -- account-balance --interval 2.0 -c polygon -a 0x75be52afd54a13b6c98490b4db495aa79b609d58,0x7caac644722316101807e0d55f838f7851a97031,0x52a258ed593c793251a89bfd36cae158ee9fc4f8,0x04e2dc96afecdf72221882e1cee039cab4d443e0,0xa32a0edc623d86e623f58e7c4174023a80a67ddf,0x7cb53b925a79fb15c348fcfd9abcf2287854d33a,0x8cf88c473b6cb40b8d37cdd93e6c8118c14a6e60,0xa96d3f3e177687fb0b5f990d5c4000923b49430b,0x92fb36230b50a87a39ba3237c996caf5a39b230b,0x0c4d7a995aa9846ef25e1a347a8711c8b534b5a6,0x698076ae39e7e44bcd2bbe15f0486c8d44bb4e6f
```


# Example attestation


http://deposit.dev.golem.network:15555/erc20/api/attestation/sepolia/0xeb9b088871155d0ae32f382de5a42d0a64e946f512b722698f4ae6b32164f92d
http://deposit.dev.golem.network:15555/erc20/api/attestation/sepolia/0xc8b0ceee393cdcf313945d20b3bd45a01b0ccf2484309b669da2d4da9266b4d5
http://deposit.dev.golem.network:15555/erc20/api/attestation/base/0xc0f18976a498f7287562492cca4a145108e83e3606e020f10653afd3511656ef
