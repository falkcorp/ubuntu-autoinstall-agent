-- file: crates/uaa-control/migrations/0001_init.sql
-- version: 1.0.0
-- guid: 01127f07-d165-4361-9ff3-8efa399a3117
-- last-edited: 2026-07-10
--
-- Normative registry schema (spec docs/specs/constellation-design.md data-model
-- section, copied verbatim). Ten table-creation statements. Versioning is handled by
-- the schema_migrations table applied in db::migrations::apply — these statements
-- are plain (no IF NOT EXISTS) so a re-apply against an already-migrated database is
-- a bug the migrations table prevents, not something SQL silently swallows.
CREATE TABLE machines (
  mac            STRING PRIMARY KEY,            -- normalized aa:bb:cc:dd:ee:ff
  hostname       STRING NOT NULL,
  ip             STRING,
  type           STRING NOT NULL DEFAULT 'lenovo',
  status         STRING NOT NULL DEFAULT 'pending',  -- pending|approved|revoked
  boot_target    STRING NOT NULL DEFAULT 'local-disk',
                 -- authoritative next-boot intent (Decision 13):
                 -- local-disk|custom-autoinstall|pxe-disabled|pxe-grub
  tpm_ek         STRING,                        -- sha256 of TPM EK pub, bound at first checkin
  registered_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  approved_at    TIMESTAMPTZ,
  last_seen      TIMESTAMPTZ,
  last_ip        STRING,
  installed_at   TIMESTAMPTZ,                   -- parity: persist install completion
  last_install_status STRING,                   -- success|failed|in-progress
  updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE install_history (
  event_id UUID PRIMARY KEY,                    -- minted at INGEST (WAL-replay dedup key)
  mac STRING NOT NULL REFERENCES machines (mac),
  started_at TIMESTAMPTZ, finished_at TIMESTAMPTZ,
  status STRING NOT NULL, detail JSONB
);
CREATE TABLE enrollments (
  spki_fingerprint STRING PRIMARY KEY,           -- sha256 of CSR public key
  mac STRING REFERENCES machines (mac),
  csr_pem STRING NOT NULL,
  state STRING NOT NULL DEFAULT 'pending',       -- pending|approved|issued|rejected|revoked|superseded
  cert_pem STRING, requested_at TIMESTAMPTZ NOT NULL DEFAULT now(), decided_by STRING
);
CREATE TABLE yubikeys (                          -- extends today's GPG/SSH registry
  fingerprint STRING PRIMARY KEY, gpg_pubkey STRING, ssh_pubkey STRING,
  comment STRING, serial STRING, status STRING NOT NULL DEFAULT 'pending',
  registered_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE luks_credentials (                  -- NEW: FIDO2 keyslot tracking
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  mac STRING NOT NULL REFERENCES machines (mac),
  yubikey_serial STRING NOT NULL,
  role STRING NOT NULL,                          -- primary|backup1|backup2
  luks_keyslot INT, enrolled_at TIMESTAMPTZ, revoked_at TIMESTAMPTZ
);
CREATE TABLE tang_servers (
  hostname STRING PRIMARY KEY, ip STRING, tang_url STRING,
  adv_keys JSONB, last_seen TIMESTAMPTZ
);
CREATE TABLE discovered_macs (                   -- uaa-pxe inbox
  mac STRING PRIMARY KEY, first_seen TIMESTAMPTZ, last_seen TIMESTAMPTZ,
  arch_hint STRING, vendor_class STRING, dismissed BOOL NOT NULL DEFAULT false
);
CREATE TABLE audit_events (
  seq INT8 PRIMARY KEY DEFAULT unique_rowid(),
  at TIMESTAMPTZ NOT NULL DEFAULT now(),
  actor STRING NOT NULL, role STRING NOT NULL,   -- github login / 'system'
  action STRING NOT NULL, target STRING, outcome STRING NOT NULL,
  detail JSONB, prev_hash BYTES NOT NULL, hash BYTES NOT NULL
  -- append serialized via SELECT tip FOR UPDATE in the recording txn (Decision 21);
  -- genesis prev_hash = 32 zero bytes
);
CREATE TABLE audit_checkpoints (
  day DATE PRIMARY KEY, tip_seq INT8 NOT NULL, tip_hash BYTES NOT NULL,
  signature BYTES NOT NULL                       -- ed25519, on-server audit key
);
CREATE TABLE saga_log (
  saga_id UUID PRIMARY KEY, kind STRING NOT NULL,
  state STRING NOT NULL,  -- running|done|compensating|compensated|compensation_pending
  steps JSONB NOT NULL, started_at TIMESTAMPTZ, finished_at TIMESTAMPTZ
);
