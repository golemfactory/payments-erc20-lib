
ALTER TABLE token_transfer RENAME COLUMN allocation_id TO deposit_id;
ALTER TABLE token_transfer DROP COLUMN use_internal;
