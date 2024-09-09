DELETE FROM chain_tx;
CREATE UNIQUE INDEX "idx_chain_tx_tx_hash" ON "chain_tx" ("tx_hash");
CREATE INDEX "idx_chain_tx_blockchain_date" ON "chain_tx" ("blockchain_date");
CREATE INDEX "idx_chain_transfer_blockchain_date" ON "chain_transfer" ("blockchain_date");
CREATE INDEX "idx_chain_transfer_receiver_address" ON "chain_transfer" ("receiver_addr");
CREATE INDEX "idx_chain_transfer_from_address" ON "chain_transfer" ("from_addr");

