use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, ErrorCode, OptionalExtension, Row, params};
use serde_json::{Value, json};
use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::atomic::ensure_private_dir;
use tagteam_provider::{Identity, ProcessStamp};

const SCHEMA_V1: &str = include_str!("schema.sql");

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
    pub started_at: i64,
    /// The unresolved row a forced switch superseded. If the forced switch never lands, this
    /// is what recovery or rollback puts back, so the unresolved state is never forgotten.
    pub prior: Option<Box<JournalRow>>,
}

/// The metadata an explicit replacement installs with its credential (§12.5). It is recorded
/// with the marker, so the next lock holder can finish a replacement whose vault write landed.
pub struct LoginMeta<'a> {
    pub identity_key: &'a str,
    pub identity: &'a Identity,
    pub kind: &'a str,
    pub login_expires_at: Option<i64>,
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
        "started_at": j.started_at,
        "prior": j.prior.as_deref().map(journal_to_json),
    })
}

fn journal_from_json(v: &Value) -> Option<JournalRow> {
    Some(JournalRow {
        provider: ProviderId::new(v["provider"].as_str()?),
        holder: ProcessStamp {
            pid: v["holder_pid"].as_u64()? as u32,
            start: v["holder_start"].as_u64()?,
        },
        from_id: v["from_id"].as_str().map(AccountId::from_string),
        to_id: AccountId::from_string(v["to_id"].as_str()?),
        from_fp: v["from_fp"].as_str().map(str::to_owned),
        from_identity: Some(v["from_identity"].clone()).filter(|x| !x.is_null()),
        to_fp: v["to_fp"].as_str()?.to_owned(),
        started_at: v["started_at"].as_i64()?,
        prior: v
            .get("prior")
            .filter(|x| !x.is_null())
            .and_then(journal_from_json)
            .map(Box::new),
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

pub struct Store {
    conn: Mutex<Connection>,
}

const ACCOUNT_COLUMNS: &str = "id, provider, position, identity_key, label, email, org_uuid, org_name, \
    account_uuid, kind, alias, disabled, identity_json, login_expires_at, login_epoch, replacing_fp, \
    quarantine_reason, quarantine_fp, added_at";

/// Installs a login's identity fields and clears any quarantine (§9.3, §12.5): shared by
/// `update_login` and the replacement `finish_replacement` records, which land the same fields.
const APPLY_LOGIN_SQL: &str = "UPDATE accounts SET identity_key = ?2, label = ?3, email = ?4, org_uuid = ?5, \
    org_name = ?6, account_uuid = COALESCE(?7, account_uuid), kind = ?8, identity_json = ?9, \
    login_expires_at = ?10, quarantine_reason = NULL, quarantine_fp = NULL, quarantine_at = NULL WHERE id = ?1";

/// Upserts the provider's active account: shared by `set_active` and `commit_switch`, which
/// upsert the same row as part of a larger transaction.
const SET_ACTIVE_SQL: &str = "INSERT INTO active_accounts (provider, account_id) VALUES (?1, ?2) \
    ON CONFLICT(provider) DO UPDATE SET account_id = excluded.account_id";

/// Clears the provider's journal row: shared by `commit_switch`, which clears it as part of
/// landing a switch, and `delete_journal`.
const DELETE_JOURNAL_SQL: &str = "DELETE FROM switch_journal WHERE provider = ?1";

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
        started_at: r.get("started_at")?,
        prior: json_col(r, "prior")?
            .as_ref()
            .and_then(journal_from_json)
            .map(Box::new),
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

fn set_active_on(
    c: &Connection,
    provider: &ProviderId,
    id: Option<&AccountId>,
) -> rusqlite::Result<usize> {
    c.execute(
        SET_ACTIVE_SQL,
        params![provider.as_str(), id.map(AccountId::as_str)],
    )
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent() {
            ensure_private_dir(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_millis(5000))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;",
        )?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_existing(path: &Path) -> Result<Option<Self>, StoreError> {
        if path.exists() {
            Self::open(path).map(Some)
        } else {
            Ok(None)
        }
    }

    fn migrate(&self) -> Result<(), StoreError> {
        let mut c = self.conn.lock().unwrap();
        let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            let tx = c.transaction()?;
            tx.execute_batch(SCHEMA_V1)?;
            tx.execute_batch("PRAGMA user_version = 1;")?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64, StoreError> {
        Ok(self
            .conn
            .lock()
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    fn query_accounts(
        &self,
        where_clause: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<AccountRow>, StoreError> {
        let c = self.conn.lock().unwrap();
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
        let c = self.conn.lock().unwrap();
        let max: Option<u32> = c.query_row(
            "SELECT MAX(position) FROM accounts WHERE provider = ?1",
            [provider.as_str()],
            |r| r.get(0),
        )?;
        Ok(max.unwrap_or(0) + 1)
    }

    pub fn insert_account(&self, a: &NewAccount<'_>) -> Result<(), StoreError> {
        let c = self.conn.lock().unwrap();
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
        Ok(self.conn.lock().unwrap().execute(sql, p)?)
    }

    pub fn update_login(
        &self,
        id: &AccountId,
        identity_key: &str,
        identity: &Identity,
        kind: &str,
        login_expires_at: Option<i64>,
    ) -> Result<(), StoreError> {
        let c = self.conn.lock().unwrap();
        let n = apply_login(&c, id, identity_key, identity, kind, login_expires_at)?;
        if n == 0 {
            Err(StoreError::NoSuchAccount)
        } else {
            Ok(())
        }
    }

    pub fn begin_replacement(
        &self,
        id: &AccountId,
        fp: &str,
        meta: &LoginMeta<'_>,
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
        });
        self.exec(
            "UPDATE accounts SET login_epoch = login_epoch + 1, replacing_fp = ?2, replacing_meta = ?3 WHERE id = ?1",
            &[&id.as_str(), &fp, &meta.to_string()],
        )?;
        Ok(())
    }

    /// The replacement landed: installs its recorded metadata, clears any quarantine and the
    /// marker, all in one transaction.
    pub fn finish_replacement(&self, id: &AccountId) -> Result<(), StoreError> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        let meta: Option<String> = tx
            .query_row(
                "SELECT replacing_meta FROM accounts WHERE id = ?1",
                [id.as_str()],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if let Some(m) = meta {
            let v: Value =
                serde_json::from_str(&m).map_err(|e| StoreError::Corrupt(e.to_string()))?;
            let identity = Identity {
                label: v["label"].as_str().unwrap_or_default().to_owned(),
                email: v["email"].as_str().map(str::to_owned),
                org_uuid: v["org_uuid"].as_str().unwrap_or_default().to_owned(),
                org_name: v["org_name"].as_str().map(str::to_owned),
                account_uuid: v["account_uuid"].as_str().map(str::to_owned),
                raw: v["identity_json"].clone(),
            };
            let identity_key = v["identity_key"].as_str().unwrap_or_default();
            let kind = v["kind"].as_str().unwrap_or_default();
            let login_expires_at = v["login_expires_at"].as_i64();
            apply_login(&tx, id, identity_key, &identity, kind, login_expires_at)?;
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

    pub fn set_alias(&self, id: &AccountId, alias: Option<&str>) -> Result<(), StoreError> {
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
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
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
        tx.execute(
            "UPDATE accounts SET position = ?2 WHERE id = ?1",
            params![id.as_str(), position],
        )?;
        if let Some(other) = &occupant {
            tx.execute(
                "UPDATE accounts SET position = ?2 WHERE id = ?1",
                params![other, from],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_account(&self, id: &AccountId) -> Result<(), StoreError> {
        self.exec("DELETE FROM accounts WHERE id = ?1", &[&id.as_str()])?;
        Ok(())
    }

    pub fn active(&self, provider: &ProviderId) -> Result<Option<AccountId>, StoreError> {
        let c = self.conn.lock().unwrap();
        let id: Option<Option<String>> = c
            .query_row(
                "SELECT account_id FROM active_accounts WHERE provider = ?1",
                [provider.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(id.flatten().map(AccountId::from_string))
    }

    pub fn set_active(
        &self,
        provider: &ProviderId,
        id: Option<&AccountId>,
    ) -> Result<(), StoreError> {
        set_active_on(&self.conn.lock().unwrap(), provider, id)?;
        Ok(())
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

    /// §9.4 step 9: the active account, the event and the journal row move together.
    pub fn commit_switch(
        &self,
        provider: &ProviderId,
        to: &AccountId,
        event: &EventRow,
    ) -> Result<(), StoreError> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        set_active_on(&tx, provider, Some(to))?;
        Self::insert_event_on(&tx, event)?;
        tx.execute(DELETE_JOURNAL_SQL, [provider.as_str()])?;
        tx.commit()?;
        Ok(())
    }

    pub fn insert_event(&self, e: &EventRow) -> Result<(), StoreError> {
        Self::insert_event_on(&self.conn.lock().unwrap(), e)?;
        Ok(())
    }

    pub fn events(&self) -> Result<Vec<EventRow>, StoreError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c.prepare("SELECT * FROM events ORDER BY rowid")?;
        let rows = stmt
            .query_map([], event_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn journal(&self, provider: &ProviderId) -> Result<Option<JournalRow>, StoreError> {
        let c = self.conn.lock().unwrap();
        Ok(c.query_row(
            "SELECT * FROM switch_journal WHERE provider = ?1",
            [provider.as_str()],
            journal_from_row,
        )
        .optional()?)
    }

    pub fn journals(&self) -> Result<Vec<JournalRow>, StoreError> {
        let c = self.conn.lock().unwrap();
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
        self.conn.lock().unwrap().execute(
            "INSERT OR REPLACE INTO switch_journal \
             (provider, holder_pid, holder_start, from_id, to_id, from_fp, from_identity, to_fp, started_at, prior) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                j.provider.as_str(),
                j.holder.pid,
                j.holder.start as i64,
                j.from_id.as_ref().map(AccountId::as_str),
                j.to_id.as_str(),
                j.from_fp,
                j.from_identity.as_ref().map(Value::to_string),
                j.to_fp,
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
        self.conn.lock().unwrap().execute(
            "INSERT INTO displaced (id, provider, at, reason, fingerprint, identity) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![d.id, d.provider.as_str(), d.at, d.reason, d.fingerprint, d.identity.as_ref().map(Value::to_string)],
        )?;
        Ok(())
    }
}
