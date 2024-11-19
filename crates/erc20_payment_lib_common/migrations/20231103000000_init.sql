DROP TABLE IF EXISTS allowance;
DROP TABLE IF EXISTS chain_transfer;
DROP TABLE IF EXISTS chain_tx;
DROP TABLE IF EXISTS scan_info;
DROP TABLE IF EXISTS token_transfer;
DROP TABLE IF EXISTS transfer_in;
DROP TABLE IF EXISTS tx;

CREATE TABLE "tx"
(
    id                  INTEGER     NOT NULL     PRIMARY KEY AUTOINCREMENT,
    method              TEXT        NOT NULL,
    from_addr           TEXT        NOT NULL,
    to_addr             TEXT        NOT NULL,
    chain_id            INTEGER     NOT NULL,
    gas_limit           INTEGER     NULL,
    max_fee_per_gas     TEXT        NULL,
    priority_fee        TEXT        NULL,
    val                 TEXT        NOT NULL,
    nonce               INTEGER     NULL,
    processing          INTEGER     NOT NULL,
    call_data           TEXT        NULL,
    created_date        TEXT        NOT NULL,
    first_processed     TEXT        NULL,
    tx_hash             TEXT        NULL,
    signed_raw_data     TEXT        NULL,
    signed_date         TEXT        NULL,
    broadcast_date      TEXT        NULL,
    broadcast_count     INTEGER     NOT NULL,
    first_stuck_date    TEXT        NULL,
    confirm_date        TEXT        NULL,
    blockchain_date     TEXT        NULL,
    gas_used            INTEGER     NULL,
    block_number        INTEGER     NULL,
    chain_status        INTEGER     NULL,
    block_gas_price     TEXT        NULL,
    effective_gas_price TEXT        NULL,
    fee_paid            TEXT        NULL,
    error               TEXT        NULL,
    orig_tx_id          INTEGER     NULL
) strict;

CREATE INDEX "idx_tx_created_date" ON "tx" (created_date);
CREATE INDEX "idx_tx_first_processed" ON "tx" (first_processed);
CREATE INDEX "idx_tx_processing" ON "tx" (processing);

CREATE TABLE "token_transfer"
(
    id                  INTEGER     NOT NULL     PRIMARY KEY AUTOINCREMENT,
    payment_id          TEXT        NULL,
    from_addr           TEXT        NOT NULL,
    receiver_addr       TEXT        NOT NULL,
    chain_id            INTEGER     NOT NULL,
    token_addr          TEXT        NULL,
    token_amount        TEXT        NOT NULL,
    create_date         TEXT        NOT NULL,
    paid_date           TEXT        NULL,
    tx_id               INTEGER     NULL,
    fee_paid            TEXT        NULL,
    error               TEXT        NULL,
    CONSTRAINT "fk_token_transfer_tx" FOREIGN KEY ("tx_id") REFERENCES "tx" ("id")
) strict;

CREATE TABLE "allowance"
(
    id                  INTEGER     NOT NULL     PRIMARY KEY AUTOINCREMENT,
    owner               TEXT        NOT NULL,
    token_addr          TEXT        NULL,
    spender             TEXT        NOT NULL,
    allowance           TEXT        NOT NULL,
    chain_id            INTEGER     NOT NULL,
    tx_id               INTEGER     NULL,
    fee_paid            TEXT        NULL,
    confirm_date        TEXT        NULL,
    error               TEXT        NULL,
    CONSTRAINT "fk_allowance_tx" FOREIGN KEY ("tx_id") REFERENCES "tx" ("id")
) strict;

CREATE TABLE "chain_tx"
(
    id                  INTEGER     NOT NULL     PRIMARY KEY AUTOINCREMENT,
    tx_hash             TEXT        NOT NULL,
    method              TEXT        NOT NULL,
    from_addr           TEXT        NOT NULL,
    to_addr             TEXT        NOT NULL,
    chain_id            INTEGER     NOT NULL,
    gas_used            INTEGER     NULL,
    gas_limit           INTEGER     NULL,
    block_gas_price     TEXT        NULL,
    effective_gas_price TEXT        NULL,
    max_fee_per_gas     TEXT        NULL,
    priority_fee        TEXT        NULL,
    val                 TEXT        NOT NULL,
    nonce               INTEGER     NOT NULL,
    checked_date        TEXT        NOT NULL,
    blockchain_date     TEXT        NOT NULL,
    block_number        INTEGER     NOT NULL,
    chain_status        INTEGER     NOT NULL,
    fee_paid            TEXT        NULL,
    error               TEXT        NULL,
    balance_eth         TEXT        NULL,
    balance_glm         TEXT        NULL
) strict;


CREATE TABLE "transfer_in"
(
    id                  INTEGER     NOT NULL     PRIMARY KEY AUTOINCREMENT,
    payment_id          TEXT        NOT NULL,
    from_addr           TEXT        NOT NULL,
    receiver_addr       TEXT        NOT NULL,
    chain_id            INTEGER     NOT NULL,
    token_addr          TEXT        NULL,
    token_amount        TEXT        NOT NULL,
    tx_hash             TEXT        NULL,
    requested_date      TEXT        NOT NULL,
    received_date       TEXT        NULL
) strict;

CREATE TABLE "chain_transfer"
(
    id                  INTEGER     NOT NULL     PRIMARY KEY AUTOINCREMENT,
    from_addr           TEXT        NOT NULL,
    receiver_addr       TEXT        NOT NULL,
    chain_id            INTEGER     NOT NULL,
    token_addr          TEXT        NULL,
    token_amount        TEXT        NOT NULL,
    chain_tx_id         INTEGER     NOT NULL,
    fee_paid            TEXT        NOT NULL,
    blockchain_date     TEXT        NOT NULL,
    CONSTRAINT "fk_chain_transfer_tx" FOREIGN KEY ("chain_tx_id") REFERENCES "chain_tx" ("id")
) strict;

CREATE TABLE "scan_info"
(
    id                  INTEGER     NOT NULL     PRIMARY KEY AUTOINCREMENT,
    chain_id            INTEGER     NOT NULL,
    filter              TEXT        NOT NULL,
    start_block         INTEGER     NOT NULL,
    last_block          INTEGER     NOT NULL
) strict;

CREATE UNIQUE INDEX "idx_scan_info_chain_id" ON "scan_info" ("chain_id", "filter");

