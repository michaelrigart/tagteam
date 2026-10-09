//! §13.3's import in the engine (Task 8, Decision 13): pass 1 validates everything before
//! anything exists, and pass 2 replaces only through §12.5's explicit replacement.

mod common;

use common::{FakeFx, Fx, crashed_switch, credential, quiescent, two_accounts};
use std::os::unix::fs::PermissionsExt;

use serde_json::{Value, json};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::export::ExportRequest;
use tagteam_engine::import::{ImportReport, Outcome};
use tagteam_engine::store::{AccountRow, Activation};
use tagteam_engine::transfer::{ImportRecord, decode, read_envelope};
use tagteam_provider::profile::Seed;
use tagteam_provider::{Provider, Read};

/// A Claude Code record for `email`'s login with refresh token `rt`.
fn record(position: u32, email: &str, rt: &str) -> ImportRecord {
    ImportRecord {
        provider: ProviderId::new(CLAUDE_CODE),
        position,
        kind: Some("oauth".into()),
        label: Some(email.into()),
        alias: None,
        disabled: false,
        added_at: Some(1_756_713_600_000),
        identity: json!({"email": email, "oauthAccount": Fx::oauth_account(email)}),
        credential: json!({"claudeAiOauth": Fx::credential_json(email, rt)["claudeAiOauth"]}),
    }
}

fn import(fx: &Fx, records: Vec<ImportRecord>, force: bool) -> ImportReport {
    fx.engine.import(records, force).unwrap()
}

/// `(position, outcome, message)` of each account of the report.
fn outcomes(r: &ImportReport) -> Vec<(u32, Outcome, String)> {
    r.accounts
        .iter()
        .map(|a| (a.position, a.outcome, a.message.clone()))
        .collect()
}

fn row(fx: &Fx, email: &str) -> AccountRow {
    fx.engine.resolve(email, None).unwrap()
}

fn events(fx: &Fx, kind: &str, id: &AccountId) -> usize {
    fx.engine
        .store()
        .unwrap()
        .events()
        .unwrap()
        .iter()
        .filter(|e| e.kind == kind && e.to_id.as_ref() == Some(id))
        .count()
}

#[test]
fn a_pass_one_refusal_names_the_account_and_creates_nothing() {
    let bad = |edit: &dyn Fn(&mut ImportRecord)| {
        let mut r = record(2, "b@x.co", "rt-b");
        edit(&mut r);
        vec![record(1, "a@x.co", "rt-a"), r]
    };
    let cases: Vec<(Vec<ImportRecord>, &str, &str)> = vec![
        (
            bad(&|r| r.provider = ProviderId::new("codex")),
            "unknown-provider",
            "unknown provider \"codex\"",
        ),
        (
            bad(&|r| r.identity = json!({"oauthAccount": Fx::oauth_account("nope")})),
            "invalid-input",
            "account 2 of the file: the identity's email is not a valid email address",
        ),
        (
            bad(&|r| r.kind = Some("bearer".into())),
            "invalid-input",
            "account 2 of the file: its kind is not a Claude Code credential kind",
        ),
        (
            bad(&|r| r.kind = Some("api_key".into())),
            "invalid-input",
            "account 2 of the file: its kind does not match its credential's, oauth",
        ),
        (
            bad(&|r| r.alias = Some("-dev".into())),
            "invalid-input",
            "account 2 of the file: its alias: an alias cannot start with '-'",
        ),
        (
            bad(&|r| r.position = 0),
            "invalid-input",
            "account 2 of the file: its position must be at least 1",
        ),
        (
            bad(&|r| r.identity = json!({"oauthAccount": Fx::oauth_account("a@x.co")})),
            "invalid-input",
            "account 2 of the file: it is the same login as an earlier account",
        ),
    ];
    for (records, kind, message) in cases {
        let fx = Fx::new();
        let err = fx.engine.import(records, false).unwrap_err();
        assert_eq!((err.kind(), err.to_string().as_str()), (kind, message));
        assert!(
            !fx.env.data_dir().exists(),
            "{message}: nothing was created"
        );
    }
    let fx = Fx::new();
    let mut a = record(1, "a@x.co", "rt-a");
    let mut b = record(2, "b@x.co", "rt-b");
    a.alias = Some("Dev".into());
    b.alias = Some("dev".into());
    let err = fx.engine.import(vec![a, b], false).unwrap_err();
    assert_eq!(
        err.to_string(),
        "account 2 of the file: its alias is an earlier account's"
    );
}

#[test]
fn a_new_login_is_created_where_the_file_put_it_and_nothing_is_made_active() {
    let fx = Fx::new();
    let mut r = record(5, "c@x.co", "rt-c");
    r.alias = Some("dev".into());
    r.disabled = true;
    let report = import(&fx, vec![r], false);
    assert_eq!(outcomes(&report), [(5, Outcome::Created, "added".into())]);
    let c = row(&fx, "c@x.co");
    assert_eq!(
        (c.position, c.alias.as_deref(), c.disabled),
        (5, Some("dev"), true)
    );
    assert_eq!(
        (c.added_at, c.login_epoch, c.kind.as_str()),
        (1_756_713_600_000, 0, "oauth")
    );
    assert_eq!(fx.vault_refresh_token(&c.id).as_deref(), Some("rt-c"));
    assert_eq!(fx.activation(), None, "an import makes nothing live (B.65)");
    assert_eq!(events(&fx, "import", &c.id), 1);
    assert_eq!(
        fx.live_refresh_token(),
        None,
        "the live login is never touched"
    );
}

#[test]
fn a_taken_position_falls_to_the_next_and_an_alias_held_here_is_dropped() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.engine.set_alias(&a, Some("dev")).unwrap();
    let mut r = record(1, "c@x.co", "rt-c");
    r.alias = Some("dev".into());
    let report = import(&fx, vec![r], false);
    assert_eq!(
        outcomes(&report),
        [(
            2,
            Outcome::Created,
            "added at position 2: 1 is not free here".into()
        )]
    );
    assert_eq!(
        report.warnings,
        [
            "the alias of the file's account at position 1 is another account's here, so the account was imported without it"
        ]
    );
    assert_eq!(row(&fx, "c@x.co").alias, None);
    assert_eq!(row(&fx, "a@x.co").alias.as_deref(), Some("dev"));
}

#[test]
fn an_existing_login_is_skipped_unless_forced_and_keeps_its_place_when_replaced() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.engine.set_alias(&a, Some("main")).unwrap();
    fx.engine.set_disabled(&a, true).unwrap();
    let mut r = record(7, "a@x.co", "rt-a9");
    r.alias = Some("other".into());

    let report = import(&fx, vec![r.clone()], false);
    assert_eq!(
        outcomes(&report),
        [(
            1,
            Outcome::Skipped,
            "already stored; pass --force to replace it".into()
        )]
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));

    let report = import(&fx, vec![r], true);
    assert_eq!(
        outcomes(&report),
        [(1, Outcome::Replaced, "replaced".into())]
    );
    let after = row(&fx, "a@x.co");
    assert_eq!(
        (
            after.position,
            after.alias.as_deref(),
            after.disabled,
            after.login_epoch
        ),
        (1, Some("main"), true, 1),
        "its local position, alias and disabled flag; its login epoch moved first (§12.5)"
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a9"));
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[test]
fn a_quarantined_login_is_replaced_without_force_with_one_unquarantine_event() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.quarantine(&a, "invalid_grant", &common::fp(&fx, "rt-a"));
    let report = import(&fx, vec![record(1, "a@x.co", "rt-a9")], false);
    assert_eq!(
        outcomes(&report),
        [(
            1,
            Outcome::Replaced,
            "replaced; its quarantine was cleared".into()
        )]
    );
    assert_eq!(row(&fx, "a@x.co").quarantine_reason, None);
    assert_eq!(
        events(&fx, "unquarantine", &a),
        1,
        "§7.4: one event per clear"
    );
}

#[test]
fn importing_over_the_live_login_stale_marks_it_and_the_old_lineage_is_never_captured_back() {
    // Review Focus 2: Claude Code keeps the old login and goes on rotating it. A switch away
    // displaces that lineage instead of capturing it over the import.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let b = row(&fx, "b@x.co").id; // live, position 2
    let report = import(&fx, vec![record(9, "b@x.co", "rt-b-new")], true);
    assert_eq!(
        report.warnings,
        [
            "position 2 is Claude Code's live login, which keeps its old login until you run `tagteam switch 2 --force`"
        ]
    );
    assert!(fx.live_store_stale(&b));
    assert_eq!(
        fx.live_refresh_token().as_deref(),
        Some("rt-b"),
        "the live store is not written"
    );
    fx.rotate_live("rt-b2");

    fx.switch_to(&a, false).unwrap();

    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b-new"));
    let displaced = fx.displaced();
    assert_eq!(displaced.len(), 1);
    assert!(String::from_utf8_lossy(&displaced[0]).contains("rt-b2"));
}

#[test]
fn switch_force_activates_an_import_over_the_live_login() {
    let fx = Fx::new();
    two_accounts(&fx);
    let b = row(&fx, "b@x.co").id;
    import(&fx, vec![record(2, "b@x.co", "rt-b-new")], true);
    fx.rotate_live("rt-b2");

    let out = fx.switch_to(&b, true).unwrap();

    assert!(out.switched);
    assert!(!fx.live_store_stale(&b));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b-new"));
}

#[test]
fn a_session_owned_account_replaced_stale_marks_its_profile_and_the_session_keeps_its_login() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let dir = quiescent(&fx, &a, "rt-a", &credential("a@x.co", "rt-a"));
    let _session = fx.hold_reservation(&dir);
    let before = fx.profile_credential(&dir);

    let report = import(&fx, vec![record(1, "a@x.co", "rt-a-new")], true);

    assert_eq!(
        report.warnings,
        [
            "position 1 is in use by a `tagteam run` session; whatever runs there keeps its login until it exits, and the next `run` starts with the imported one"
        ]
    );
    let Read::Present(seed) = Seed::read(&dir) else {
        panic!("the seed is left as it was")
    };
    assert_ne!(
        seed.login_epoch,
        row(&fx, "a@x.co").login_epoch,
        "stale-marked (§12.5)"
    );
    assert_eq!(
        fx.profile_credential(&dir),
        before,
        "a running profile is never touched"
    );
}

#[test]
fn an_id_and_the_active_account_in_the_file_are_never_used() {
    // B.65, through the envelope reader: a file's `id` and `active` are never read.
    let fx = Fx::new();
    let file = json!({"format": "tagteam-export", "version": 1, "active": {"claude-code": 1},
        "accounts": [{"id": "0192-from-elsewhere", "provider": "claude-code", "position": 1,
            "kind": "oauth", "label": "a@x.co", "alias": null, "disabled": false,
            "addedAt": null, "identity": record(1, "a@x.co", "rt-a").identity,
            "credential": record(1, "a@x.co", "rt-a").credential}]});
    import(&fx, read_envelope(&file).unwrap(), false);
    let a = row(&fx, "a@x.co");
    assert_ne!(a.id.as_str(), "0192-from-elsewhere");
    assert_eq!(fx.activation(), None);
}

#[test]
fn import_refuses_inside_a_run_shell_and_while_a_switch_is_undecidable() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let dir = fx.make_profile(&a);
    let shell = fx.engine_located(fx.shell_env(&dir));
    let err = shell
        .import(vec![record(3, "c@x.co", "rt-c")], false)
        .unwrap_err();
    assert_eq!(err.kind(), "inside-run-shell");

    let b = row(&fx, "b@x.co").id;
    crashed_switch(&fx, &b, &a);
    fx.set_live_credential(&credential("x@x.co", "rt-x")); // neither side's: undecidable
    let err = fx
        .engine
        .import(vec![record(1, "a@x.co", "rt-a9")], true)
        .unwrap_err();
    assert_eq!(err.kind(), "interrupted-switch");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
}

#[test]
fn a_failure_on_one_account_is_reported_and_the_others_go_on() {
    let fx = Fx::new();
    two_accounts(&fx); // a@x.co has accountUuid uuid-a@x.co
    let mut other = record(1, "a@x.co", "rt-a9");
    other.identity["oauthAccount"]["accountUuid"] = json!("uuid-someone-else");
    let report = import(&fx, vec![other, record(3, "c@x.co", "rt-c")], true);
    let got = outcomes(&report);
    assert_eq!((got[0].0, got[0].1), (1, Outcome::Failed));
    assert!(
        got[0].2.contains("belongs to a different account"),
        "{}",
        got[0].2
    );
    assert_eq!(got[1], (3, Outcome::Created, "added".into()));
    assert!(report.any_failed());
}

#[test]
fn an_export_imports_into_another_home_with_its_positions_aliases_and_flags() {
    let from = Fx::new();
    let a = two_accounts(&from);
    from.engine.set_alias(&a, Some("main")).unwrap();
    let exported = from.engine.export(&ExportRequest::default()).unwrap();
    let decoded = decode(&exported.envelope, &[], &mut |_| None).unwrap();

    let to = Fx::new();
    let report = import(&to, decoded.records, false);

    assert_eq!(
        outcomes(&report),
        [
            (1, Outcome::Created, "added".into()),
            (2, Outcome::Created, "added".into())
        ]
    );
    let a2 = row(&to, "main");
    assert_eq!(a2.email.as_deref(), Some("a@x.co"));
    assert_eq!(to.vault_refresh_token(&a2.id).as_deref(), Some("rt-a"));
    let v: Value = serde_json::from_slice(&to.vault_bytes(&a2.id).unwrap()).unwrap();
    assert!(
        v.get("mcpOAuth").is_none(),
        "a slim export carries no machine-shared key"
    );
    assert_eq!(to.activation(), None);
}

#[test]
fn a_fake_agent_login_imports_through_its_own_payload() {
    let ff = FakeFx::new();
    let r = ImportRecord {
        provider: ff.fake_provider(),
        position: 1,
        kind: Some("fa_token".into()),
        label: None,
        alias: None,
        disabled: false,
        added_at: None,
        identity: tagteam_fake::identity_json("alice", "ws", "uid-alice"),
        credential: json!({"fa": {"token": "tok-a", "renew": "renew-a"}}),
    };
    let report = ff.engine.import(vec![r], false).unwrap();
    assert_eq!(report.accounts[0].outcome, Outcome::Created);
    assert_eq!(
        report.accounts[0].email, "alice@ws",
        "its label: it has no email"
    );
    let stored = ff
        .engine
        .store()
        .unwrap()
        .accounts(&ff.fake_provider())
        .unwrap();
    assert_eq!(stored[0].kind, "fa_token");
    assert_eq!(
        ff.fake.fingerprint(
            &ff.fx
                .kc
                .get(tagteam_engine::vault::SERVICE, stored[0].id.as_str())
                .unwrap()
        ),
        Some(tagteam_core::Fingerprint::of_secret(b"renew-a"))
    );
}

#[test]
fn a_file_with_no_account_imports_nothing_and_creates_nothing() {
    let fx = Fx::new();
    assert_eq!(import(&fx, vec![], false), ImportReport::default());
    assert!(!fx.env.data_dir().exists());
}

#[test]
fn a_replacement_records_the_live_identity_as_evidence_whatever_the_store_says_is_active() {
    // §12.5 "A replacement records its own evidence": Claude Code was logged in as `a` by hand
    // (`claude /login`), so `active_accounts` names another account or none. The import reads
    // the live identity, so the live store is still stale-marked for `a` at its old epoch.
    for named in [Some("b@x.co"), None] {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let provider = fx.provider();
        let store = fx.engine.store().unwrap();
        match named {
            Some(email) => store
                .set_active(&provider, Some(&row(&fx, email).id), Some(0))
                .unwrap(),
            None => store.set_active(&provider, None, None).unwrap(),
        }
        fx.login("a@x.co", "rt-a-live");
        assert_ne!(fx.activation().map(|x| x.account), Some(a.clone()));

        let report = import(&fx, vec![record(1, "a@x.co", "rt-a9")], true);

        assert_eq!(
            outcomes(&report),
            [(1, Outcome::Replaced, "replaced".into())]
        );
        assert_eq!(
            fx.activation(),
            Some(Activation {
                account: a.clone(),
                epoch: Some(0)
            }),
            "named {named:?}: the account at the epoch it had before the replacement"
        );
        assert!(fx.live_store_stale(&a), "named {named:?}");
        assert_eq!(
            report.warnings,
            [
                "position 1 is Claude Code's live login, which keeps its old login until you run `tagteam switch 1 --force`"
            ]
        );
    }
}

#[test]
fn an_unreadable_live_identity_fails_that_account_and_writes_nothing_for_it() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let before = row(&fx, "a@x.co");
    let config = fx.paths().global_config;
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o000)).unwrap();

    let result = fx.engine.import(vec![record(1, "a@x.co", "rt-a9")], true);

    // Restored before any assertion can panic and leave the tempdir unreadable for cleanup.
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    let report = result.unwrap();
    assert!(report.any_failed());
    assert_eq!(report.accounts[0].outcome, Outcome::Failed);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    let after = row(&fx, "a@x.co");
    assert_eq!(
        (after.login_epoch, after.replacing_fp.clone()),
        (before.login_epoch, None),
        "no replacement was begun"
    );
    assert_eq!(events(&fx, "import", &a), 0);
}
