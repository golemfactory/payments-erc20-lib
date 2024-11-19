DROP TABLE IF EXISTS "old_table";
DROP TABLE IF EXISTS "new_table";

CREATE TABLE "old_table"
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
    error               TEXT        NULL
) strict;

INSERT INTO old_table SELECT * FROM token_transfer;

DROP TABLE token_transfer;

CREATE TABLE token_transfer
(
    id                  INTEGER     NOT NULL     PRIMARY KEY AUTOINCREMENT,
    payment_id          TEXT        NULL,
    from_addr           TEXT        NOT NULL,
    receiver_addr       TEXT        NOT NULL,
    chain_id            INTEGER     NOT NULL,
    token_addr          TEXT        NULL,
    token_amount        TEXT        NOT NULL,
    allocation_id       TEXT        NULL,
    use_internal        INTEGER     NOT NULL DEFAULT 0,
    create_date         TEXT        NOT NULL,
    paid_date           TEXT        NULL,
    tx_id               INTEGER     NULL,
    fee_paid            TEXT        NULL,
    error               TEXT        NULL,
    CONSTRAINT "fk_token_transfer_tx" FOREIGN KEY ("tx_id") REFERENCES "tx" ("id")
) strict;

INSERT INTO token_transfer(id, payment_id, from_addr, receiver_addr, chain_id, token_addr, token_amount, create_date, paid_date, tx_id, fee_paid, error) SELECT id, payment_id, from_addr, receiver_addr, chain_id, token_addr, token_amount, create_date, paid_date, tx_id, fee_paid, error FROM old_table;
