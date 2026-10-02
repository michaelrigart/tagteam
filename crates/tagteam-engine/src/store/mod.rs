use std::fs::{OpenOptions, Permissions};
use std::io;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use rusqlite::{
    Connection, ErrorCode, OpenFlags, OptionalExtension, Row, TransactionBehavior, params,
};
use serde_json::{Value, json};
use tagteam_core::autoswitch::{AutoState, Departure, Trigger};
use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::atomic::ensure_private_dir;
use tagteam_provider::{Identity, ProcessStamp};

mod usage;

pub use usage::{
    Eligibility, Ineligible, LiveIdentityCacheRow, Reservation, Reserve, SendGrant, Slot,
    UsageStateRow,
};
pub(crate) use usage::{backoff_holds, backoff_is_skewed, plan_is_skewed};

/// Version 1, frozen: never edited, so a fresh file and an upgraded one end in the same schema.
const SCHEMA_V1: &str = include_str!("schema.sql");

/// Version 2 (§6.1, §12.5): the activation epoch on `active_accounts`, filled with each named
/// account's current `login_epoch` (the live store is taken as current, the best evidence there
/// is), and the target's epoch on the switch journal, left NULL on a row written before it
/// (§9.6 falls back to the target's current epoch).
const MIGRATION_V2: &str = "ALTER TABLE active_accounts ADD COLUMN login_epoch INTEGER;
ALTER TABLE switch_journal ADD COLUMN to_epoch INTEGER;
UPDATE active_accounts SET login_epoch =
  (SELECT login_epoch FROM accounts WHERE accounts.id = active_accounts.account_id)
  WHERE account_id IS NOT NULL;";

/// The migration ladder: step `n` takes a file from version `n` to `n + 1`. A fresh file runs
/// every step, and an older one only the steps above its version.
const MIGRATIONS: [&str; 2] = [SCHEMA_V1, MIGRATION_V2];

/// The `PRAGMA user_version` this build knows how to read and write. A stored version above
/// this is a store written by a newer tagteam; `migrate` refuses it rather than guessing.
const SCHEMA_VERSION: i64 = 2;

const _: () = assert!(MIGRATIONS.len() as i64 == SCHEMA_VERSION);

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("store error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("store I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("the store holds invalid data: {0}")]
    Corrupt(String),
    #[error("the alias {0:?} is already taken")]
    AliasTaken(String),
    #[error("position {0} is already taken")]
    PositionTaken(u32),
    #[error("that login is already stored")]
    IdentityTaken,
    #[error("no such account")]
    NoSuchAccount,
    #[error("store schema v{0} is newer than this tagteam supports")]
    UnsupportedSchema(i64),
    #[error("a replacement is already pending for this account")]
    ReplacementPending,
    #[error("an alias cannot be empty")]
    InvalidAlias,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountRow {
    pub id: AccountId,
    pub provider: ProviderId,
    pub position: u32,
    pub identity_key: String,
    pub label: String,
    pub email: Option<String>,
    pub org_uuid: String,
    pub org_name: Option<String>,
    pub account_uuid: Option<String>,
    pub kind: String,
    pub alias: Option<String>,
    pub disabled: bool,
    pub identity_json: Value,
    pub login_expires_at: Option<i64>,
    pub login_epoch: i64,
    pub replacing_fp: Option<String>,
    pub quarantine_reason: Option<String>,
    pub quarantine_fp: Option<String>,
    pub quarantine_at: Option<i64>,
    pub added_at: i64,
}

pub struct NewAccount<'a> {
    pub id: &'a AccountId,
    pub provider: &'a ProviderId,
    pub position: u32,
    pub identity_key: &'a str,
    pub identity: &'a Identity,
    pub kind: &'a str,
    pub alias: Option<&'a str>,
    pub login_expires_at: Option<i64>,
    pub added_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventRow {
    pub at: i64,
    pub provider: ProviderId,
    pub kind: String,
    pub from_id: Option<AccountId>,
    pub to_id: Option<AccountId>,
    pub trigger: Option<String>,
    pub source: String,
    pub detail: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JournalRow {
    pub provider: ProviderId,
    pub holder: ProcessStamp,
    pub from_id: Option<AccountId>,
    pub to_id: AccountId,
    pub from_fp: Option<String>,
    pub from_identity: Option<Value>,
    pub to_fp: String,
    /// The target's `login_epoch` when the row was written (§9.4 step 6): the activation epoch
    /// a forward finish records (§9.6). `None` on a row written before the column existed.
    pub to_epoch: Option<i64>,
    pub started_at: i64,
    /// The unresolved row a forced switch superseded. If the forced switch never lands, this
    /// is what recovery or rollback puts back, so the unresolved state is never forgotten.
    pub prior: Option<Box<JournalRow>>,
}

/// What an automatic switch's commit records for its provider (§9.4 step 9, §11.2 step 11):
/// when it switched, in epoch seconds, between which accounts, and the departure snapshot of
/// the account it left (§11.3).
#[derive(Debug, Clone, PartialEq)]
pub struct AutoRecord {
    pub at: i64,
    pub from: AccountId,
    pub to: AccountId,
    pub departure: Departure,
}

/// The metadata an explicit replacement installs with its credential (§12.5). It is recorded
/// with the marker, so the next lock holder can finish a replacement whose vault write landed.
pub struct LoginMeta<'a> {
    pub identity_key: &'a str,
    pub identity: &'a Identity,
    pub kind: &'a str,
    pub login_expires_at: Option<i64>,
    /// The login was taken from the live store (`add`, §10.1). Once it lands, the live store
    /// holds exactly the vault's generation, so finishing records the new epoch as the
    /// activation epoch (§12.5). Recorded in `replacing_meta`, so a reconciliation does too.
    pub from_live: bool,
}

/// The store's record of the default home's live login (§12.5): the account tagteam made live,
/// and that account's `login_epoch` when it did, or from before a replacement superseded the
/// live login. `epoch` is `None` only for a row written without one; the migration filled every
/// named account's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activation {
    pub account: AccountId,
    pub epoch: Option<i64>,
}

fn journal_to_json(j: &JournalRow) -> Value {
    json!({
        "provider": j.provider.as_str(),
        "holder_pid": j.holder.pid,
        "holder_start": j.holder.start,
        "from_id": j.from_id.as_ref().map(AccountId::as_str),
        "to_id": j.to_id.as_str(),
        "from_fp": j.from_fp,
        "from_identity": j.from_identity,
        "to_fp": j.to_fp,
        "to_epoch": j.to_epoch,
        "started_at": j.started_at,
        "prior": j.prior.as_deref().map(journal_to_json),
    })
}

/// The inverse of `journal_to_json`. Returns an error rather than silently dropping a
/// malformed `prior` snapshot: a caller (rollback or recovery) that got `None` back would
/// delete the undecidable row instead of restoring it (§9.6). A snapshot written before
/// `to_epoch` existed has no such key, and reads as `None`.
fn journal_from_json(v: &Value) -> rusqlite::Result<JournalRow> {
    fn malformed() -> rusqlite::Error {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            "malformed journal snapshot".into(),
        )
    }
    Ok(JournalRow {
        provider: ProviderId::new(v["provider"].as_str().ok_or_else(malformed)?),
        holder: ProcessStamp {
            pid: v["holder_pid"].as_u64().ok_or_else(malformed)? as u32,
            start: v["holder_start"].as_u64().ok_or_else(malformed)?,
        },
        from_id: v["from_id"].as_str().map(AccountId::from_string),
        to_id: AccountId::from_string(v["to_id"].as_str().ok_or_else(malformed)?),
        from_fp: v["from_fp"].as_str().map(str::to_owned),
        from_identity: Some(v["from_identity"].clone()).filter(|x| !x.is_null()),
        to_fp: v["to_fp"].as_str().ok_or_else(malformed)?.to_owned(),
        to_epoch: v["to_epoch"].as_i64(),
        started_at: v["started_at"].as_i64().ok_or_else(malformed)?,
        prior: match v.get("prior") {
            Some(p) if !p.is_null() => Some(Box::new(journal_from_json(p)?)),
            _ => None,
        },
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct DisplacedRow {
    pub id: String,
    pub provider: ProviderId,
    pub at: i64,
    pub reason: String,
    pub fingerprint: String,
    pub identity: Option<Value>,
}

/// §12.7: a directory mapped to an account, for one provider. `path` is stored as given; the
/// CLI gives the canonical path (§12.7). `added_at` is epoch ms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    pub path: String,
    pub provider: ProviderId,
    pub account_id: AccountId,
    pub added_at: i64,
}

const MAPPING_COLUMNS: &str = "path, provider, account_id, added_at";

fn mapping_from_row(r: &Row<'_>) -> rusqlite::Result<Mapping> {
    Ok(Mapping {
        path: r.get("path")?,
        provider: ProviderId::new(r.get::<_, String>("provider")?),
        account_id: AccountId::from_string(r.get::<_, String>("account_id")?),
        added_at: r.get("added_at")?,
    })
}

pub struct Store {
    conn: Mutex<Connection>,
}

const ACCOUNT_COLUMNS: &str = "id, provider, position, identity_key, label, email, org_uuid, org_name, \
    account_uuid, kind, alias, disabled, identity_json, login_expires_at, login_epoch, replacing_fp, \
    quarantine_reason, quarantine_fp, quarantine_at, added_at";

/// Installs a login's identity fields and clears any quarantine: shared by `update_login` and
/// the replacement `finish_replacement` records, which land the same fields. Clearing here is
/// §7.4's rule, not an exception to it. Every caller installs a login that replaces the vault's:
/// `add`, `add-token` and `import` clear a quarantine explicitly, and the switch's outgoing
/// capture only ever writes a generation whose fingerprint differs from the vault's (it is not
/// `Ours`, §9.4 step 4).
const APPLY_LOGIN_SQL: &str = "UPDATE accounts SET identity_key = ?2, label = ?3, email = ?4, org_uuid = ?5, \
    org_name = ?6, account_uuid = COALESCE(?7, account_uuid), kind = ?8, identity_json = ?9, \
    login_expires_at = ?10, quarantine_reason = NULL, quarantine_fp = NULL, quarantine_at = NULL WHERE id = ?1";

/// Upserts the provider's active account and its activation epoch, both columns on conflict:
/// an epoch left from the previous account would stale-mark the next one (§12.5). Shared by
/// every writer of the row, each inside its own larger transaction or alone.
const SET_ACTIVE_SQL: &str = "INSERT INTO active_accounts (provider, account_id, login_epoch) \
    VALUES (?1, ?2, ?3) \
    ON CONFLICT(provider) DO UPDATE SET account_id = excluded.account_id, \
    login_epoch = excluded.login_epoch";

/// Clears the provider's journal row: shared by `commit_switch`, which clears it as part of
/// landing a switch, and `delete_journal`.
const DELETE_JOURNAL_SQL: &str = "DELETE FROM switch_journal WHERE provider = ?1";

/// Writes an automatic switch's record over the provider's `autoswitch_state` row, creating it
/// if needed, and resets `unhealthy_ticks`: the count judged the account the switch left
/// (Decision 4). `idle_hold_since` is never written (§6.1).
const RECORD_SWITCH_SQL: &str = "INSERT INTO autoswitch_state (provider, last_switch_at, \
    last_switch_from, last_switch_to, left_headroom, left_recovery_at, left_trigger, unhealthy_ticks) \
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0) ON CONFLICT(provider) DO UPDATE SET \
    last_switch_at = excluded.last_switch_at, last_switch_from = excluded.last_switch_from, \
    last_switch_to = excluded.last_switch_to, left_headroom = excluded.left_headroom, \
    left_recovery_at = excluded.left_recovery_at, left_trigger = excluded.left_trigger, \
    unhealthy_ticks = 0";

/// Moves one account to a given position: shared by both sides of the swap in `move_to`.
const SET_POSITION_SQL: &str = "UPDATE accounts SET position = ?2 WHERE id = ?1";

fn json_col(r: &Row<'_>, name: &str) -> rusqlite::Result<Option<Value>> {
    let raw: Option<String> = r.get(name)?;
    raw.map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })
}

fn account_from_row(r: &Row<'_>) -> rusqlite::Result<AccountRow> {
    Ok(AccountRow {
        id: AccountId::from_string(r.get::<_, String>("id")?),
        provider: ProviderId::new(r.get::<_, String>("provider")?),
        position: r.get("position")?,
        identity_key: r.get("identity_key")?,
        label: r.get("label")?,
        email: r.get("email")?,
        org_uuid: r.get("org_uuid")?,
        org_name: r.get("org_name")?,
        account_uuid: r.get("account_uuid")?,
        kind: r.get("kind")?,
        alias: r.get("alias")?,
        disabled: r.get::<_, i64>("disabled")? != 0,
        identity_json: json_col(r, "identity_json")?.unwrap_or(Value::Null),
        login_expires_at: r.get("login_expires_at")?,
        login_epoch: r.get("login_epoch")?,
        replacing_fp: r.get("replacing_fp")?,
        quarantine_reason: r.get("quarantine_reason")?,
        quarantine_fp: r.get("quarantine_fp")?,
        quarantine_at: r.get("quarantine_at")?,
        added_at: r.get("added_at")?,
    })
}

fn journal_from_row(r: &Row<'_>) -> rusqlite::Result<JournalRow> {
    Ok(JournalRow {
        provider: ProviderId::new(r.get::<_, String>("provider")?),
        holder: ProcessStamp {
            pid: r.get("holder_pid")?,
            start: r.get::<_, i64>("holder_start")? as u64,
        },
        from_id: r
            .get::<_, Option<String>>("from_id")?
            .map(AccountId::from_string),
        to_id: AccountId::from_string(r.get::<_, String>("to_id")?),
        from_fp: r.get("from_fp")?,
        from_identity: json_col(r, "from_identity")?,
        to_fp: r.get("to_fp")?,
        to_epoch: r.get("to_epoch")?,
        started_at: r.get("started_at")?,
        prior: match json_col(r, "prior")? {
            Some(v) => Some(Box::new(journal_from_json(&v)?)),
            None => None,
        },
    })
}

fn event_from_row(r: &Row<'_>) -> rusqlite::Result<EventRow> {
    Ok(EventRow {
        at: r.get("at")?,
        provider: ProviderId::new(r.get::<_, String>("provider")?),
        kind: r.get("kind")?,
        from_id: r
            .get::<_, Option<String>>("from_id")?
            .map(AccountId::from_string),
        to_id: r
            .get::<_, Option<String>>("to_id")?
            .map(AccountId::from_string),
        trigger: r.get("trigger")?,
        source: r.get("source")?,
        detail: json_col(r, "detail")?,
    })
}

/// Maps UNIQUE violations to named errors.
fn classify(e: rusqlite::Error, position: u32, alias: Option<&str>) -> StoreError {
    if let rusqlite::Error::SqliteFailure(f, Some(msg)) = &e {
        if f.code == ErrorCode::ConstraintViolation {
            if msg.contains("accounts.alias") {
                return StoreError::AliasTaken(alias.unwrap_or_default().to_owned());
            }
            if msg.contains("accounts.position") {
                return StoreError::PositionTaken(position);
            }
            if msg.contains("accounts.identity_key") {
                return StoreError::IdentityTaken;
            }
        }
    }
    StoreError::Sqlite(e)
}

/// Runs `APPLY_LOGIN_SQL` against any connection-like handle (a plain connection or a
/// transaction), returning the number of rows changed.
fn apply_login(
    c: &Connection,
    id: &AccountId,
    identity_key: &str,
    identity: &Identity,
    kind: &str,
    login_expires_at: Option<i64>,
) -> rusqlite::Result<usize> {
    c.execute(
        APPLY_LOGIN_SQL,
        params![
            id.as_str(),
            identity_key,
            identity.label,
            identity.email,
            identity.org_uuid,
            identity.org_name,
            identity.account_uuid,
            kind,
            identity.raw.to_string(),
            login_expires_at,
        ],
    )
}

/// `SET_ACTIVE_SQL` on any connection-like handle. No account means no epoch, whatever the
/// caller passes (§6.1: NULL only with `account_id`).
fn set_active_on(
    c: &Connection,
    provider: &ProviderId,
    id: Option<&AccountId>,
    epoch: Option<i64>,
) -> rusqlite::Result<usize> {
    c.execute(
        SET_ACTIVE_SQL,
        params![provider.as_str(), id.map(AccountId::as_str), id.and(epoch)],
    )
}

/// Reads `PRAGMA user_version`, shared by `migrate` (both the pre-check and the re-check made
/// under its transaction) and `schema_version`.
fn user_version(c: &Connection) -> rusqlite::Result<i64> {
    c.query_row("PRAGMA user_version", [], |r| r.get(0))
}

/// How long a connection waits on another's lock before it gives up.
const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);

/// The flags every tagteam connection opens with, `SQLITE_OPEN_CREATE` only when the caller
/// may create the file. `open_existing` opens without it, so a database removed between its
/// existence check and this call is never recreated out from under the removal.
fn open_flags(create: bool) -> OpenFlags {
    let base = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if create {
        base | OpenFlags::SQLITE_OPEN_CREATE
    } else {
        base
    }
}

/// `PRAGMA journal_mode = WAL` needs a brief exclusive lock to convert a database that has
/// never been in WAL mode, and — unlike ordinary reads and writes — reports `SQLITE_BUSY`
/// immediately rather than waiting on the connection's busy handler (sqlite.org/pragma.html
/// #pragma_journal_mode). Concurrent first opens of the same fresh file must therefore retry
/// it themselves; this polls for the same 5-second budget as `busy_timeout` below.
fn set_wal_mode(conn: &Connection) -> Result<(), StoreError> {
    let deadline = Instant::now() + Duration::from_millis(5000);
    loop {
        match conn.pragma_update(None, "journal_mode", "WAL") {
            Ok(()) => return Ok(()),
            Err(rusqlite::Error::SqliteFailure(f, _))
                if f.code == ErrorCode::DatabaseBusy && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Opens the connection and its connection-scoped pragmas only. Deliberately does **not**
/// switch the file to WAL: that is a persistent, on-disk mutation, and `migrate` must be free
/// to refuse a newer-schema file before anything touches it at all.
fn connect(path: &Path, create: bool) -> Result<Connection, StoreError> {
    let conn = Connection::open_with_flags(path, open_flags(create))?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")?;
    Ok(conn)
}

fn is_cannot_open(e: &StoreError) -> bool {
    matches!(e, StoreError::Sqlite(rusqlite::Error::SqliteFailure(f, _)) if f.code == ErrorCode::CannotOpen)
}

/// Creates the database file with mode 0600 before SQLite first opens it (§6.1): it names
/// every account. SQLite gives its `-wal` and `-shm` files the database's own mode. A file
/// that already exists is left exactly as it is; an empty one is a valid empty database.
fn create_private(path: &Path) -> io::Result<()> {
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        // At creation, before any byte, so the umask can never widen it.
        Ok(file) => file.set_permissions(Permissions::from_mode(0o600)),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent() {
            ensure_private_dir(dir)?;
        }
        create_private(path)?;
        let conn = connect(path, true)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Never creates the database file itself when absent: it checks existence first, then
    /// opens without `SQLITE_OPEN_CREATE`, so a database removed in between is reported as
    /// absent rather than recreated. (Its WAL/SHM sidecars do appear while the store is open
    /// and are removed again on close; this only concerns the main file.)
    ///
    /// A `CannotOpen` failure is reported as absent only if the file has genuinely disappeared
    /// since the check above: re-confirmed here rather than assumed, so an existing file this
    /// process simply cannot read (wrong permissions, a stale root-owned file, ...) is reported
    /// as an error instead of silently treated as "no store".
    pub fn open_existing(path: &Path) -> Result<Option<Self>, StoreError> {
        if !path.try_exists()? {
            return Ok(None);
        }
        let conn = match connect(path, false) {
            Ok(conn) => conn,
            Err(e) if is_cannot_open(&e) => {
                if path.try_exists()? {
                    return Err(e);
                }
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(Some(store))
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Applies each migration step above the file's version exactly once (`MIGRATIONS`). A
    /// stored version newer than `SCHEMA_VERSION` is refused outright, before anything else
    /// runs against the connection — including switching it to WAL, which is why that pragma is
    /// set here and not in `connect`: a file this build refuses must be left exactly as it was
    /// found. The check-and-apply itself runs in one `IMMEDIATE` transaction, re-reading the
    /// version under the write lock it grants — both to guard against a concurrent racing
    /// first open (§6.1), and, re-checked again, against a concurrent newer binary upgrading
    /// the file between this function's first read and the moment it takes the lock. Every step
    /// and the version bump commit together, so a file is never left between two versions. WAL
    /// mode is set only once every such check has passed and, when a step ran, only after that
    /// transaction has committed — WAL can't be switched from inside a transaction, and a file
    /// this build ends up refusing must never have been touched at all.
    fn migrate(&self) -> Result<(), StoreError> {
        let mut c = self.lock();
        let version = user_version(&c)?;
        if version > SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchema(version));
        }
        if version < SCHEMA_VERSION {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let version = user_version(&tx)?;
            if version > SCHEMA_VERSION {
                return Err(StoreError::UnsupportedSchema(version));
            }
            if version < SCHEMA_VERSION {
                let done = usize::try_from(version).unwrap_or(0);
                for step in &MIGRATIONS[done..] {
                    tx.execute_batch(step)?;
                }
                tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
            }
            tx.commit()?;
        }
        set_wal_mode(&c)?;
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64, StoreError> {
        Ok(user_version(&self.lock())?)
    }

    fn query_accounts(
        &self,
        where_clause: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<AccountRow>, StoreError> {
        let c = self.lock();
        let sql = format!("SELECT {ACCOUNT_COLUMNS} FROM accounts {where_clause}");
        let mut stmt = c.prepare(&sql)?;
        let rows = stmt
            .query_map(params, account_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn one(
        &self,
        where_clause: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Option<AccountRow>, StoreError> {
        Ok(self
            .query_accounts(where_clause, params)?
            .into_iter()
            .next())
    }

    pub fn accounts(&self, provider: &ProviderId) -> Result<Vec<AccountRow>, StoreError> {
        self.query_accounts(
            "WHERE provider = ?1 ORDER BY position",
            &[&provider.as_str()],
        )
    }

    pub fn all_accounts(&self) -> Result<Vec<AccountRow>, StoreError> {
        self.query_accounts("ORDER BY provider, position", &[])
    }

    pub fn account(&self, id: &AccountId) -> Result<Option<AccountRow>, StoreError> {
        self.one("WHERE id = ?1", &[&id.as_str()])
    }

    pub fn find_by_identity_key(
        &self,
        provider: &ProviderId,
        key: &str,
    ) -> Result<Option<AccountRow>, StoreError> {
        self.one(
            "WHERE provider = ?1 AND identity_key = ?2",
            &[&provider.as_str(), &key],
        )
    }

    pub fn find_by_position(
        &self,
        provider: &ProviderId,
        position: u32,
    ) -> Result<Option<AccountRow>, StoreError> {
        self.one(
            "WHERE provider = ?1 AND position = ?2",
            &[&provider.as_str(), &position],
        )
    }

    pub fn find_by_alias(&self, alias: &str) -> Result<Option<AccountRow>, StoreError> {
        if alias.is_empty() {
            return Ok(None);
        }
        self.one("WHERE alias = ?1", &[&alias])
    }

    pub fn find_by_email(
        &self,
        email: &str,
        provider: Option<&ProviderId>,
    ) -> Result<Vec<AccountRow>, StoreError> {
        match provider {
            Some(p) => self.query_accounts(
                "WHERE email = ?1 AND provider = ?2 ORDER BY position",
                &[&email, &p.as_str()],
            ),
            None => self.query_accounts("WHERE email = ?1 ORDER BY provider, position", &[&email]),
        }
    }

    pub fn next_position(&self, provider: &ProviderId) -> Result<u32, StoreError> {
        let c = self.lock();
        let max: Option<u32> = c.query_row(
            "SELECT MAX(position) FROM accounts WHERE provider = ?1",
            [provider.as_str()],
            |r| r.get(0),
        )?;
        max.unwrap_or(0).checked_add(1).ok_or_else(|| {
            StoreError::Corrupt(format!(
                "provider {:?} has no position left to allocate",
                provider.as_str()
            ))
        })
    }

    pub fn insert_account(&self, a: &NewAccount<'_>) -> Result<(), StoreError> {
        let c = self.lock();
        c.execute(
            "INSERT INTO accounts (id, provider, position, identity_key, label, email, org_uuid, org_name, \
             account_uuid, kind, alias, identity_json, login_expires_at, added_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                a.id.as_str(),
                a.provider.as_str(),
                a.position,
                a.identity_key,
                a.identity.label,
                a.identity.email,
                a.identity.org_uuid,
                a.identity.org_name,
                a.identity.account_uuid,
                a.kind,
                a.alias,
                a.identity.raw.to_string(),
                a.login_expires_at,
                a.added_at,
            ],
        )
        .map_err(|e| classify(e, a.position, a.alias))?;
        Ok(())
    }

    fn exec(&self, sql: &str, p: &[&dyn rusqlite::ToSql]) -> Result<usize, StoreError> {
        Ok(self.lock().execute(sql, p)?)
    }

    pub fn update_login(
        &self,
        id: &AccountId,
        identity_key: &str,
        identity: &Identity,
        kind: &str,
        login_expires_at: Option<i64>,
    ) -> Result<(), StoreError> {
        let c = self.lock();
        let n = apply_login(&c, id, identity_key, identity, kind, login_expires_at)?;
        if n == 0 {
            Err(StoreError::NoSuchAccount)
        } else {
            Ok(())
        }
    }

    /// Marks the start of a replacement (§12.5 step 1): bumps the epoch and records both the
    /// incoming fingerprint and the metadata to install once it lands. Guarded so a second
    /// `begin` on an already-pending account is refused rather than clobbering the first.
    ///
    /// The same transaction records the default home's evidence. When `live_names_account`
    /// (the engine read the live identity) or `active_accounts` names the account, and the row
    /// does not already name it with an epoch, the row is set to the account at the epoch it
    /// had before the increment. The live store is then stale-marked whoever activated it and
    /// whatever the row held before; for a login taken from the live store, `finish_replacement`
    /// lifts the mark. A rollback restores that same epoch, so it needs no undo here.
    pub fn begin_replacement(
        &self,
        id: &AccountId,
        fp: &str,
        meta: &LoginMeta<'_>,
        live_names_account: bool,
    ) -> Result<(), StoreError> {
        let meta = json!({
            "identity_key": meta.identity_key,
            "label": meta.identity.label,
            "email": meta.identity.email,
            "org_uuid": meta.identity.org_uuid,
            "org_name": meta.identity.org_name,
            "account_uuid": meta.identity.account_uuid,
            "kind": meta.kind,
            "identity_json": meta.identity.raw,
            "login_expires_at": meta.login_expires_at,
            "from_live": meta.from_live,
        });
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let n = tx.execute(
            "UPDATE accounts SET login_epoch = login_epoch + 1, replacing_fp = ?2, replacing_meta = ?3 \
             WHERE id = ?1 AND replacing_fp IS NULL",
            params![id.as_str(), fp, meta.to_string()],
        )?;
        if n == 0 {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE id = ?1)",
                [id.as_str()],
                |r| r.get(0),
            )?;
            return Err(if exists {
                StoreError::ReplacementPending
            } else {
                StoreError::NoSuchAccount
            });
        }
        let (provider, epoch): (String, i64) = tx.query_row(
            "SELECT provider, login_epoch FROM accounts WHERE id = ?1",
            [id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let provider = ProviderId::new(provider);
        let active: Option<(Option<String>, Option<i64>)> = tx
            .query_row(
                "SELECT account_id, login_epoch FROM active_accounts WHERE provider = ?1",
                [provider.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (names, with_epoch) = match &active {
            Some((Some(named), recorded)) if named == id.as_str() => (true, recorded.is_some()),
            _ => (false, false),
        };
        if (live_names_account || names) && !with_epoch {
            set_active_on(&tx, &provider, Some(id), Some(epoch - 1))?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The replacement landed: installs its recorded metadata, clears any quarantine and the
    /// marker, all in one transaction. A missing `identity_key`, `label` or `kind` in the
    /// recorded metadata means the account and its marker are left exactly as they were
    /// (§12.5) rather than installing an empty identity.
    ///
    /// A login taken from the live store (`from_live`, `add`'s) also records the account's new
    /// `login_epoch` as the activation epoch here (§10.1, §12.5): the live store holds exactly
    /// the vault's generation. Only while `active_accounts` still names the account:
    /// `begin_replacement` made it so, and a switch that committed another account after a
    /// replacer died is the newer record. Metadata without `from_live` is not from the live
    /// store.
    pub fn finish_replacement(&self, id: &AccountId) -> Result<(), StoreError> {
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row: Option<Option<String>> = tx
            .query_row(
                "SELECT replacing_meta FROM accounts WHERE id = ?1",
                [id.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        let meta = match row {
            None => return Err(StoreError::NoSuchAccount),
            Some(meta) => meta,
        };
        if let Some(m) = meta {
            let v: Value =
                serde_json::from_str(&m).map_err(|e| StoreError::Corrupt(e.to_string()))?;
            let missing = |field: &str| {
                StoreError::Corrupt(format!("replacement metadata is missing its {field} field"))
            };
            let identity_key = v["identity_key"]
                .as_str()
                .ok_or_else(|| missing("identity_key"))?;
            let label = v["label"].as_str().ok_or_else(|| missing("label"))?;
            let kind = v["kind"].as_str().ok_or_else(|| missing("kind"))?;
            let identity = Identity {
                label: label.to_owned(),
                email: v["email"].as_str().map(str::to_owned),
                org_uuid: v["org_uuid"].as_str().unwrap_or_default().to_owned(),
                org_name: v["org_name"].as_str().map(str::to_owned),
                account_uuid: v["account_uuid"].as_str().map(str::to_owned),
                raw: v["identity_json"].clone(),
            };
            let login_expires_at = v["login_expires_at"].as_i64();
            apply_login(&tx, id, identity_key, &identity, kind, login_expires_at)?;
            if v["from_live"].as_bool().unwrap_or(false) {
                tx.execute(
                    "UPDATE active_accounts SET login_epoch = \
                     (SELECT login_epoch FROM accounts WHERE id = ?1) WHERE account_id = ?1",
                    [id.as_str()],
                )?;
            }
        }
        tx.execute(
            "UPDATE accounts SET replacing_fp = NULL, replacing_meta = NULL WHERE id = ?1",
            [id.as_str()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn rollback_replacement(&self, id: &AccountId) -> Result<(), StoreError> {
        self.exec(
            "UPDATE accounts SET login_epoch = login_epoch - 1, replacing_fp = NULL, replacing_meta = NULL \
             WHERE id = ?1 AND replacing_fp IS NOT NULL",
            &[&id.as_str()],
        )?;
        Ok(())
    }

    pub fn backfill_account_uuid(&self, id: &AccountId, uuid: &str) -> Result<(), StoreError> {
        self.exec(
            "UPDATE accounts SET account_uuid = ?2 WHERE id = ?1 AND account_uuid IS NULL",
            &[&id.as_str(), &uuid],
        )?;
        Ok(())
    }

    /// §7.4: one strike, bound to the fingerprint that was sent.
    pub fn set_quarantine(
        &self,
        id: &AccountId,
        reason: &str,
        fp: &str,
        at: i64,
    ) -> Result<(), StoreError> {
        let n = self.exec(
            "UPDATE accounts SET quarantine_reason = ?2, quarantine_fp = ?3, quarantine_at = ?4 \
             WHERE id = ?1",
            &[&id.as_str(), &reason, &fp, &at],
        )?;
        if n == 0 {
            Err(StoreError::NoSuchAccount)
        } else {
            Ok(())
        }
    }

    /// Clears the quarantine; `true` when there was one.
    pub fn clear_quarantine(&self, id: &AccountId) -> Result<bool, StoreError> {
        let n = self.exec(
            "UPDATE accounts SET quarantine_reason = NULL, quarantine_fp = NULL, quarantine_at = NULL \
             WHERE id = ?1 AND quarantine_reason IS NOT NULL",
            &[&id.as_str()],
        )?;
        Ok(n > 0)
    }

    /// The login's own expiry (CC: `refreshTokenExpiresAt`), after a new generation lands.
    pub fn set_login_expires_at(&self, id: &AccountId, at: Option<i64>) -> Result<(), StoreError> {
        self.exec(
            "UPDATE accounts SET login_expires_at = ?2 WHERE id = ?1",
            &[&id.as_str(), &at],
        )?;
        Ok(())
    }

    pub fn set_alias(&self, id: &AccountId, alias: Option<&str>) -> Result<(), StoreError> {
        if alias == Some("") {
            return Err(StoreError::InvalidAlias);
        }
        self.exec(
            "UPDATE accounts SET alias = ?2 WHERE id = ?1",
            &[&id.as_str(), &alias],
        )
        .map_err(|e| match e {
            StoreError::Sqlite(e) => classify(e, 0, alias),
            other => other,
        })?;
        Ok(())
    }

    pub fn set_disabled(&self, id: &AccountId, disabled: bool) -> Result<(), StoreError> {
        self.exec(
            "UPDATE accounts SET disabled = ?2 WHERE id = ?1",
            &[&id.as_str(), &(disabled as i64)],
        )?;
        Ok(())
    }

    pub fn move_to(&self, id: &AccountId, position: u32) -> Result<(), StoreError> {
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (provider, from): (String, u32) = tx
            .query_row(
                "SELECT provider, position FROM accounts WHERE id = ?1",
                [id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or(StoreError::NoSuchAccount)?;
        let occupant: Option<String> = tx
            .query_row(
                "SELECT id FROM accounts WHERE provider = ?1 AND position = ?2 AND id != ?3",
                params![provider, position, id.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(other) = &occupant {
            tx.execute("UPDATE accounts SET position = 0 WHERE id = ?1", [other])?;
        }
        tx.execute(SET_POSITION_SQL, params![id.as_str(), position])?;
        if let Some(other) = &occupant {
            tx.execute(SET_POSITION_SQL, params![other, from])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Deletes the account; its usage rows cascade, and its `usage:<id>` lease row (which has
    /// no foreign key) goes in the same transaction. `ON DELETE SET NULL` clears an
    /// `active_accounts` row that named it, but cannot clear a second column, so the orphaned
    /// activation epoch is cleared here too (§6.1: NULL only with `account_id`).
    pub fn delete_account(&self, id: &AccountId) -> Result<(), StoreError> {
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM accounts WHERE id = ?1", [id.as_str()])?;
        tx.execute(
            "DELETE FROM leases WHERE name = ?1",
            [usage::lease_name(id)],
        )?;
        tx.execute(
            "UPDATE active_accounts SET login_epoch = NULL WHERE account_id IS NULL",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn active(&self, provider: &ProviderId) -> Result<Option<AccountId>, StoreError> {
        let c = self.lock();
        let id: Option<Option<String>> = c
            .query_row(
                "SELECT account_id FROM active_accounts WHERE provider = ?1",
                [provider.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(id.flatten().map(AccountId::from_string))
    }

    /// The provider's active account and its activation epoch (§12.5); `None` when no account
    /// is recorded (none was ever set, or it was removed).
    pub fn activation(&self, provider: &ProviderId) -> Result<Option<Activation>, StoreError> {
        let c = self.lock();
        let row: Option<(Option<String>, Option<i64>)> = c
            .query_row(
                "SELECT account_id, login_epoch FROM active_accounts WHERE provider = ?1",
                [provider.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.and_then(|(id, epoch)| {
            id.map(|id| Activation {
                account: AccountId::from_string(id),
                epoch,
            })
        }))
    }

    /// `epoch` is stored with `id`; `id = None` stores `None` for both.
    pub fn set_active(
        &self,
        provider: &ProviderId,
        id: Option<&AccountId>,
        epoch: Option<i64>,
    ) -> Result<(), StoreError> {
        set_active_on(&self.lock(), provider, id, epoch)?;
        Ok(())
    }

    /// §12.5: `active_accounts` names `row` with an epoch other than `row.login_epoch`. An
    /// explicit replacement then superseded the lineage the live store holds, which is never
    /// captured. A row with no epoch is no evidence, and reads as current, as the migration
    /// takes it.
    pub fn live_store_stale(&self, row: &AccountRow) -> Result<bool, StoreError> {
        Ok(matches!(
            self.activation(&row.provider)?,
            Some(Activation { account, epoch: Some(epoch) })
                if account == row.id && epoch != row.login_epoch
        ))
    }

    fn insert_event_on(c: &Connection, e: &EventRow) -> rusqlite::Result<usize> {
        c.execute(
            "INSERT INTO events (at, provider, kind, from_id, to_id, trigger, source, detail) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                e.at,
                e.provider.as_str(),
                e.kind,
                e.from_id.as_ref().map(AccountId::as_str),
                e.to_id.as_ref().map(AccountId::as_str),
                e.trigger,
                e.source,
                e.detail.as_ref().map(Value::to_string),
            ],
        )
    }

    /// §9.4 step 9: the active account, its activation epoch, the event and the journal row move
    /// together, and so does an automatic switch's `record` (§11.2 step 11), which also resets
    /// `unhealthy_ticks`. The record is written first, so any later statement that fails
    /// takes it down too. The caller supplies `epoch` (§12.5): a switch passes the target's
    /// `login_epoch`, which cannot move while its account lock is held, and recovery (§9.6) the
    /// row's journaled `to_epoch`, so a replacement that landed since leaves the live store
    /// stale-marked.
    pub fn commit_switch(
        &self,
        provider: &ProviderId,
        to: &AccountId,
        epoch: i64,
        event: &EventRow,
        record: Option<&AutoRecord>,
    ) -> Result<(), StoreError> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        if let Some(r) = record {
            tx.execute(
                RECORD_SWITCH_SQL,
                params![
                    provider.as_str(),
                    r.at,
                    r.from.as_str(),
                    r.to.as_str(),
                    r.departure.left_headroom,
                    r.departure.left_recovery_at,
                    r.departure.left_trigger.as_str(),
                ],
            )?;
        }
        set_active_on(&tx, provider, Some(to), Some(epoch))?;
        Self::insert_event_on(&tx, event)?;
        tx.execute(DELETE_JOURNAL_SQL, [provider.as_str()])?;
        tx.commit()?;
        Ok(())
    }

    /// The provider's auto-switch state (§6.1); a provider without a row has the default. A
    /// `left_trigger` this build does not know reads as none, which lifts the no-return bar
    /// (§11.3: a missing departure snapshot lifts it).
    pub fn autoswitch_state(&self, provider: &ProviderId) -> Result<AutoState, StoreError> {
        let c = self.lock();
        let state = c
            .query_row(
                "SELECT last_switch_at, last_switch_from, last_switch_to, left_headroom, \
                 left_recovery_at, left_trigger, unhealthy_ticks \
                 FROM autoswitch_state WHERE provider = ?1",
                [provider.as_str()],
                |r| {
                    Ok(AutoState {
                        last_switch_at: r.get(0)?,
                        last_switch_from: r
                            .get::<_, Option<String>>(1)?
                            .map(AccountId::from_string),
                        last_switch_to: r.get::<_, Option<String>>(2)?.map(AccountId::from_string),
                        left_headroom: r.get(3)?,
                        left_recovery_at: r.get(4)?,
                        left_trigger: r
                            .get::<_, Option<String>>(5)?
                            .as_deref()
                            .and_then(Trigger::parse),
                        unhealthy_ticks: r.get(6)?,
                    })
                },
            )
            .optional()?;
        Ok(state.unwrap_or_default())
    }

    /// §11.2 step 5: the engine's count of ticks in a row whose active usage was unknown. The
    /// rest of the row is left as it is.
    pub fn set_unhealthy_ticks(&self, provider: &ProviderId, n: u32) -> Result<(), StoreError> {
        self.exec(
            "INSERT INTO autoswitch_state (provider, unhealthy_ticks) VALUES (?1, ?2) \
             ON CONFLICT(provider) DO UPDATE SET unhealthy_ticks = excluded.unhealthy_ticks",
            &[&provider.as_str(), &n],
        )?;
        Ok(())
    }

    pub fn insert_event(&self, e: &EventRow) -> Result<(), StoreError> {
        Self::insert_event_on(&self.lock(), e)?;
        Ok(())
    }

    pub fn events(&self) -> Result<Vec<EventRow>, StoreError> {
        let c = self.lock();
        let mut stmt = c.prepare("SELECT * FROM events ORDER BY rowid")?;
        let rows = stmt
            .query_map([], event_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn journal(&self, provider: &ProviderId) -> Result<Option<JournalRow>, StoreError> {
        let c = self.lock();
        Ok(c.query_row(
            "SELECT * FROM switch_journal WHERE provider = ?1",
            [provider.as_str()],
            journal_from_row,
        )
        .optional()?)
    }

    pub fn journals(&self) -> Result<Vec<JournalRow>, StoreError> {
        let c = self.lock();
        let mut stmt = c.prepare("SELECT * FROM switch_journal ORDER BY provider")?;
        let rows = stmt
            .query_map([], journal_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Writes the provider's journal row, atomically replacing any row already there: a
    /// forced switch settles an undecidable row this way without a gap in which neither
    /// row exists (§9.6).
    pub fn insert_journal(&self, j: &JournalRow) -> Result<(), StoreError> {
        self.lock().execute(
            "INSERT OR REPLACE INTO switch_journal \
             (provider, holder_pid, holder_start, from_id, to_id, from_fp, from_identity, to_fp, \
             to_epoch, started_at, prior) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                j.provider.as_str(),
                j.holder.pid,
                j.holder.start as i64,
                j.from_id.as_ref().map(AccountId::as_str),
                j.to_id.as_str(),
                j.from_fp,
                j.from_identity.as_ref().map(Value::to_string),
                j.to_fp,
                j.to_epoch,
                j.started_at,
                j.prior.as_deref().map(|p| journal_to_json(p).to_string()),
            ],
        )?;
        Ok(())
    }

    pub fn delete_journal(&self, provider: &ProviderId) -> Result<(), StoreError> {
        self.exec(DELETE_JOURNAL_SQL, &[&provider.as_str()])?;
        Ok(())
    }

    pub fn insert_displaced(&self, d: &DisplacedRow) -> Result<(), StoreError> {
        self.lock().execute(
            "INSERT INTO displaced (id, provider, at, reason, fingerprint, identity) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![d.id, d.provider.as_str(), d.at, d.reason, d.fingerprint, d.identity.as_ref().map(Value::to_string)],
        )?;
        Ok(())
    }

    /// §12.7: maps `path` to `account` for `provider`, replacing the mapping `path` held for
    /// that provider (one per provider per path). `account` must be an account of `provider`;
    /// otherwise nothing is written and the answer is `NoSuchAccount`.
    pub fn set_mapping(
        &self,
        path: &str,
        provider: &ProviderId,
        account: &AccountId,
        at: i64,
    ) -> Result<(), StoreError> {
        // An INSERT … SELECT finds the account and writes in one statement, so an account a
        // concurrent `remove` deletes is either mapped first, and the mapping cascades with it,
        // or not found. Its WHERE clause also keeps SQLite from reading `ON CONFLICT` as a join.
        let n = self.exec(
            "INSERT INTO mappings (path, provider, account_id, added_at) \
             SELECT ?1, provider, id, ?4 FROM accounts WHERE id = ?3 AND provider = ?2 \
             ON CONFLICT(path, provider) DO UPDATE SET account_id = excluded.account_id, \
             added_at = excluded.added_at",
            &[&path, &provider.as_str(), &account.as_str(), &at],
        )?;
        if n == 0 {
            Err(StoreError::NoSuchAccount)
        } else {
            Ok(())
        }
    }

    /// Removes `path`'s mapping for `provider`, or for every provider when `None`. Returns the
    /// count.
    pub fn remove_mappings(
        &self,
        path: &str,
        provider: Option<&ProviderId>,
    ) -> Result<usize, StoreError> {
        match provider {
            Some(p) => self.exec(
                "DELETE FROM mappings WHERE path = ?1 AND provider = ?2",
                &[&path, &p.as_str()],
            ),
            None => self.exec("DELETE FROM mappings WHERE path = ?1", &[&path]),
        }
    }

    /// Every mapping, by path, then provider.
    pub fn mappings(&self) -> Result<Vec<Mapping>, StoreError> {
        let c = self.lock();
        let mut stmt = c.prepare(&format!(
            "SELECT {MAPPING_COLUMNS} FROM mappings ORDER BY path, provider"
        ))?;
        let rows = stmt
            .query_map([], mapping_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// §12.7: the mapping of `dir` or its nearest mapped ancestor, for `provider` (`dir`
    /// canonical). Ancestors are whole path components, `dir` itself first: `/a/bc` never
    /// inherits `/a/b`'s mapping, as a string prefix would. A component that is not UTF-8 can
    /// match no stored path, so the walk passes it by and goes on to its parent.
    pub fn nearest_mapping(
        &self,
        dir: &Path,
        provider: &ProviderId,
    ) -> Result<Option<Mapping>, StoreError> {
        // Rebuilt from its components: a trailing `/` or a `.` would otherwise spell an
        // ancestor no mapping is stored under.
        let dir: PathBuf = dir.components().collect();
        let c = self.lock();
        let mut stmt = c.prepare(&format!(
            "SELECT {MAPPING_COLUMNS} FROM mappings WHERE path = ?1 AND provider = ?2"
        ))?;
        for ancestor in dir.ancestors() {
            let Some(path) = ancestor.to_str().filter(|p| !p.is_empty()) else {
                continue;
            };
            let found = stmt
                .query_row(params![path, provider.as_str()], mapping_from_row)
                .optional()?;
            if found.is_some() {
                return Ok(found);
            }
        }
        Ok(None)
    }
}
