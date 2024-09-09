-- Deposit id is interpreted as composite of deposit id and deposit contract address.
DELETE FROM token_transfer WHERE deposit_id IS NOT NULL;