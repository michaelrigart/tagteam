//! The compat store's SQLite file, which the harness opens itself for the two things tagteam
//! has no command for: the identity values a report must never carry, read before any child
//! runs, and making the test account due for an on-demand collection (§8.3), which is what
//! runs tagteam's active refresh. Only while no tagteam process runs.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, params};
use tagteam_provider::PollBudget;

use super::report::Redactor;
use super::sys::{HarnessError, harness};

fn open(db: &Path, flags: OpenFlags) -> Result<Connection, HarnessError> {
    let conn = Connection::open_with_flags(db, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| harness(format!("the compat store {}: {e}", db.display())))?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|e| harness(format!("the compat store: {e}")))?;
    Ok(conn)
}

/// Every account's email, label, organization name and uuid, and account uuid, as
/// `<account N>`, `<account N org>`, `<account N org uuid>` and `<account N uuid>`, N its
/// position.
pub fn redactor(db: &Path) -> Result<Redactor, HarnessError> {
    let conn = open(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let read = |e: rusqlite::Error| harness(format!("reading the compat store's accounts: {e}"));
    let mut stmt = conn
        .prepare("SELECT position, email, label, org_name, org_uuid, account_uuid FROM accounts")
        .map_err(read)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                [
                    (r.get::<_, Option<String>>(1)?, ""),
                    (r.get::<_, Option<String>>(2)?, ""),
                    (r.get::<_, Option<String>>(3)?, " org"),
                    (r.get::<_, Option<String>>(4)?, " org uuid"),
                    (r.get::<_, Option<String>>(5)?, " uuid"),
                ],
            ))
        })
        .map_err(read)?;
    let mut out = Redactor::default();
    for row in rows {
        let (n, values) = row.map_err(read)?;
        for (value, what) in values {
            if let Some(v) = value {
                out.learn(&v, format!("<account {n}{what}>"));
            }
        }
    }
    Ok(out)
}

/// Makes account `id` eligible for `tagteam list`'s collection now (§8.3 `OnDemand`): no poll
/// planned, no backoff, and its reading older than Claude Code's floor (`PollBudget::STANDARD`).
/// Only the reading's time moves; what it read is untouched.
pub fn make_due(db: &Path, id: &str, now_s: i64) -> Result<(), HarnessError> {
    let conn = open(db, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let stale = now_s - PollBudget::STANDARD.floor_s - 1;
    conn.execute(
        "UPDATE usage_state SET next_poll_at = NULL, backoff_until = NULL, \
         fetched_at = MIN(fetched_at, ?2) WHERE account_id = ?1",
        params![id, stale],
    )
    .map_err(|e| harness(format!("making {id} due in the compat store: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = include_str!("../../../tagteam-engine/src/store/schema.sql");

    fn store(name: &str) -> std::path::PathBuf {
        let db = std::env::temp_dir().join(format!("xtask-store-{name}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        for (id, position, email, label) in [
            ("a1", 1, Some("t@x.co"), "t@x.co"),
            ("a2", 2, None, "Setup token 7f3a"),
        ] {
            conn.execute(
                "INSERT INTO accounts (id, provider, position, identity_key, label, email, \
                 org_uuid, org_name, account_uuid, kind, identity_json, added_at) \
                 VALUES (?1, 'claude-code', ?2, ?1, ?3, ?4, 'org-uuid-9', 'Org Nine', \
                 'acct-uuid-1', 'oauth', '{}', 0)",
                params![id, position, label, email],
            )
            .unwrap();
        }
        db
    }

    #[test]
    fn every_account_s_identity_is_learned_by_position() {
        let db = store("ids");
        let r = redactor(&db).unwrap();
        assert_eq!(
            r.text("t@x.co Setup token 7f3a Org Nine org-uuid-9 acct-uuid-1"),
            "<account 1> <account 2> <account 1 org> <account 1 org uuid> <account 1 uuid>"
        );
        std::fs::remove_file(&db).unwrap();
    }

    #[test]
    fn a_due_account_has_no_plan_no_backoff_and_a_reading_past_the_floor() {
        let db = store("due");
        let now = 1_790_000_000_i64;
        let conn = Connection::open(&db).unwrap();
        for id in ["a1", "a2"] {
            conn.execute(
                "INSERT INTO usage_state (account_id, fetched_at, next_poll_at, backoff_until) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, now - 10, now + 170, now + 60],
            )
            .unwrap();
        }
        make_due(&db, "a1", now).unwrap();
        let row = |id: &str| -> (Option<i64>, Option<i64>, Option<i64>) {
            conn.query_row(
                "SELECT fetched_at, next_poll_at, backoff_until FROM usage_state \
                 WHERE account_id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
        };
        let floor = PollBudget::STANDARD.floor_s;
        assert_eq!(row("a1"), (Some(now - floor - 1), None, None));
        assert_eq!(row("a2"), (Some(now - 10), Some(now + 170), Some(now + 60)));
        make_due(&db, "a1", now + 5).unwrap();
        assert_eq!(
            row("a1").0,
            Some(now - floor - 1),
            "an older reading is kept"
        );
        std::fs::remove_file(&db).unwrap();
    }
}
