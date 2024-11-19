-- It will be better to use unique index, but it is possible that can break some functionality
CREATE INDEX "idx_payment_id" ON "token_transfer" (payment_id);