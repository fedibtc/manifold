-- The seat invite that the authenticated MG asserted with the latest admitted
-- snapshot, AEAD-encrypted under the store key. Rows written before this
-- migration have no invite until the seat's next admitted poll.
ALTER TABLE metric_snapshots ADD COLUMN invite_nonce BLOB;
ALTER TABLE metric_snapshots ADD COLUMN invite_ciphertext BLOB;
