CREATE TABLE accounts (
  id                TEXT PRIMARY KEY,
  provider          TEXT NOT NULL,
  position          INTEGER NOT NULL,
  identity_key      TEXT NOT NULL,
  label             TEXT NOT NULL,
  email             TEXT,
  org_uuid          TEXT NOT NULL DEFAULT '',
  org_name          TEXT,
  account_uuid      TEXT,
  kind              TEXT NOT NULL,
  alias             TEXT UNIQUE COLLATE NOCASE,
  disabled          INTEGER NOT NULL DEFAULT 0,
  identity_json     TEXT NOT NULL,
  login_expires_at  INTEGER,
  login_epoch       INTEGER NOT NULL DEFAULT 0,
  replacing_fp      TEXT,
  replacing_meta    TEXT,
  quarantine_reason TEXT,
  quarantine_fp     TEXT,
  quarantine_at     INTEGER,
  added_at          INTEGER NOT NULL,
  UNIQUE (provider, position),
  UNIQUE (provider, identity_key)
);

CREATE TABLE active_accounts (
  provider   TEXT PRIMARY KEY,
  account_id TEXT REFERENCES accounts(id) ON DELETE SET NULL
);

CREATE TABLE usage_state (
  account_id           TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
  last_good            TEXT,
  fetched_at           INTEGER,
  last_attempt_at      INTEGER,
  consecutive_failures INTEGER NOT NULL DEFAULT 0,
  last_error           TEXT,
  backoff_until        INTEGER,
  next_poll_at         INTEGER,
  poll_interval_s      INTEGER,
  last_429_at          INTEGER,
  rejected_fp          TEXT
);

CREATE TABLE usage_samples (
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  window     TEXT NOT NULL,
  fetched_at INTEGER NOT NULL,
  pct        REAL NOT NULL,
  resets_at  INTEGER,
  PRIMARY KEY (account_id, window, fetched_at)
) WITHOUT ROWID;

CREATE TABLE usage_requests (
  provider     TEXT NOT NULL,
  identity_key TEXT NOT NULL,
  at           INTEGER NOT NULL
);
CREATE INDEX usage_requests_by_identity ON usage_requests (provider, identity_key, at);

CREATE TABLE leases (
  name       TEXT PRIMARY KEY,
  holder     TEXT NOT NULL,
  expires_at INTEGER NOT NULL
);

CREATE TABLE switch_journal (
  provider      TEXT PRIMARY KEY,
  holder_pid    INTEGER NOT NULL,
  holder_start  INTEGER NOT NULL,
  from_id       TEXT,
  to_id         TEXT NOT NULL REFERENCES accounts(id),
  from_fp       TEXT,
  from_identity TEXT,
  to_fp         TEXT NOT NULL,
  started_at    INTEGER NOT NULL,
  prior         TEXT
);

CREATE TABLE autoswitch_state (
  provider          TEXT PRIMARY KEY,
  last_switch_at    INTEGER,
  last_switch_from  TEXT,
  last_switch_to    TEXT,
  left_headroom     REAL,
  left_recovery_at  INTEGER,
  left_trigger      TEXT,
  unhealthy_ticks   INTEGER NOT NULL DEFAULT 0,
  idle_hold_since   INTEGER
);

CREATE TABLE events (
  at       INTEGER NOT NULL,
  provider TEXT NOT NULL,
  kind     TEXT NOT NULL,
  from_id  TEXT,
  to_id    TEXT,
  trigger  TEXT,
  source   TEXT NOT NULL,
  detail   TEXT
);

CREATE TABLE mappings (
  path       TEXT NOT NULL,
  provider   TEXT NOT NULL,
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  added_at   INTEGER NOT NULL,
  PRIMARY KEY (path, provider)
);

CREATE TABLE displaced (
  id          TEXT PRIMARY KEY,
  provider    TEXT NOT NULL,
  at          INTEGER NOT NULL,
  reason      TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  identity    TEXT
);

CREATE TABLE live_identity_cache (
  provider     TEXT PRIMARY KEY,
  path         TEXT,
  mtime_ns     INTEGER,
  size         INTEGER,
  identity_key TEXT,
  label        TEXT,
  account_uuid TEXT
);
