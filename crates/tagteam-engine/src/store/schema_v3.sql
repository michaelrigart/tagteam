CREATE TABLE token_files (
  path       TEXT PRIMARY KEY,
  provider   TEXT NOT NULL,
  dev        INTEGER NOT NULL,
  ino        INTEGER NOT NULL,
  size       INTEGER NOT NULL,
  mtime_ns   INTEGER NOT NULL,
  offset     INTEGER NOT NULL,
  head_hash  BLOB NOT NULL,
  tail_hash  BLOB NOT NULL,
  indexed_at INTEGER NOT NULL
);

CREATE TABLE token_sessions (
  id         INTEGER PRIMARY KEY,
  provider   TEXT NOT NULL,
  session_id TEXT NOT NULL,
  project    TEXT,
  UNIQUE (provider, session_id)
);

CREATE TABLE token_models (
  id       INTEGER PRIMARY KEY,
  provider TEXT NOT NULL,
  name     TEXT NOT NULL,
  UNIQUE (provider, name)
);

-- `id` follows insertion, so the rows of a time range share pages; `key` is the response's
-- identity (ruling R7).
CREATE TABLE token_usage (
  id         INTEGER PRIMARY KEY,
  key        INTEGER NOT NULL UNIQUE,
  session    INTEGER NOT NULL REFERENCES token_sessions(id) ON DELETE CASCADE,
  at         INTEGER NOT NULL,
  model      INTEGER NOT NULL REFERENCES token_models(id),
  sidechain  INTEGER NOT NULL,
  input      INTEGER NOT NULL,
  output     INTEGER NOT NULL,
  cw_5m      INTEGER NOT NULL,
  cw_1h      INTEGER NOT NULL,
  cache_read INTEGER NOT NULL,
  web_search INTEGER NOT NULL,
  speed      TEXT,
  finalized  INTEGER NOT NULL
);
CREATE INDEX token_usage_by_at ON token_usage (at);
CREATE INDEX token_usage_by_session ON token_usage (session, at);

CREATE TABLE session_sightings (
  provider    TEXT NOT NULL,
  session_id  TEXT NOT NULL,
  home        TEXT NOT NULL,
  first_at    INTEGER NOT NULL,
  last_at     INTEGER NOT NULL,
  source      TEXT NOT NULL,
  recorded_at INTEGER NOT NULL,
  PRIMARY KEY (provider, session_id, home, first_at)
) WITHOUT ROWID;

CREATE TABLE run_launches (
  provider    TEXT NOT NULL,
  account_id  TEXT NOT NULL,
  started_at  INTEGER NOT NULL,
  ended_at    INTEGER,
  pid         INTEGER,
  recorded_at INTEGER NOT NULL,
  PRIMARY KEY (provider, account_id, started_at)
) WITHOUT ROWID;

CREATE TABLE active_history (
  provider    TEXT NOT NULL,
  at          INTEGER NOT NULL,
  account_id  TEXT,
  recorded_at INTEGER NOT NULL
);
CREATE INDEX active_history_by_at ON active_history (provider, at);

CREATE TRIGGER active_history_insert AFTER INSERT ON active_accounts BEGIN
  INSERT INTO active_history (provider, at, account_id, recorded_at) VALUES (
    NEW.provider,
    CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER),
    NEW.account_id,
    CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
  );
END;

CREATE TRIGGER active_history_update AFTER UPDATE OF account_id ON active_accounts
  WHEN NEW.account_id IS NOT OLD.account_id BEGIN
  INSERT INTO active_history (provider, at, account_id, recorded_at) VALUES (
    NEW.provider,
    CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER),
    NEW.account_id,
    CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
  );
END;

CREATE TABLE token_state (
  provider          TEXT PRIMARY KEY,
  refreshed_at      INTEGER,
  changed_at        INTEGER,
  files             INTEGER NOT NULL DEFAULT 0,
  responses         INTEGER NOT NULL DEFAULT 0,
  skipped_lines     INTEGER NOT NULL DEFAULT 0,
  unreadable_files  INTEGER NOT NULL DEFAULT 0,
  pruned_at         INTEGER
);

CREATE TABLE token_epoch (
  at     INTEGER NOT NULL,
  seeded INTEGER NOT NULL
);
