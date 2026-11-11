mod common;

use std::fs;
use std::path::PathBuf;

use common::{Fx, add_same_email, cc};
use tagteam_core::AccountId;
use tagteam_engine::mappings::{ConfigMapping, MappedDir, MappingProblem, Named};
use tagteam_engine::settings::config_path;

fn write_mappings(fx: &Fx, entries: &[(&str, &str)]) {
    let mut body = String::from("[provider.claude-code.mappings]\n");
    for (key, value) in entries {
        body.push_str(&format!("{key:?} = {value:?}\n"));
    }
    let path = config_path(&fx.env);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn mkdir(fx: &Fx, name: &str) -> PathBuf {
    let dir = fx.env.home.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(dir).unwrap()
}

fn judged(fx: &Fx) -> Vec<ConfigMapping> {
    fx.engine.config_mappings(&cc()).unwrap()
}

#[test]
fn alias_and_email_values_name_their_account() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.engine.set_alias(&a, Some("work")).unwrap();
    let (w, v) = (mkdir(&fx, "w"), mkdir(&fx, "v"));
    write_mappings(&fx, &[("~/w", "work"), ("~/v", "b@x.co")]);

    let got = judged(&fx);
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].dir, MappedDir::Found(w));
    assert_eq!(got[0].account.as_ref().map(|r| &r.id), Some(&a));
    assert_eq!(got[0].problem, None);
    assert_eq!(got[1].dir, MappedDir::Found(v));
    assert_eq!(got[1].account.as_ref().map(|r| &r.id), Some(&b));
    assert_eq!(got[1].problem, None);
}

#[test]
fn a_position_value_never_resolves() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    mkdir(&fx, "w");
    write_mappings(&fx, &[("~/w", "2")]);

    let got = judged(&fx);
    assert_eq!(got[0].account, None);
    assert_eq!(got[0].problem, Some(MappingProblem::Position));
}

#[test]
fn an_unknown_or_ambiguous_value_names_no_account() {
    let fx = Fx::new();
    add_same_email(&fx, "a@x.co");
    mkdir(&fx, "w");
    mkdir(&fx, "v");
    write_mappings(&fx, &[("~/w", "nobody"), ("~/v", "a@x.co")]);

    let got = judged(&fx);
    assert_eq!(got[0].account, None);
    assert_eq!(got[0].problem, Some(MappingProblem::NoAccount));
    assert_eq!(got[1].account, None);
    assert_eq!(got[1].problem, Some(MappingProblem::Ambiguous));
}

#[test]
fn an_api_key_account_is_named_but_does_not_apply() {
    let fx = Fx::new();
    let key = fx.add_api_key("sk-ant-api03-key");
    fx.engine.set_alias(&key, Some("key")).unwrap();
    mkdir(&fx, "w");
    write_mappings(&fx, &[("~/w", "key")]);

    let got = judged(&fx);
    assert_eq!(got[0].account.as_ref().map(|r| &r.id), Some(&key));
    assert_eq!(got[0].problem, Some(MappingProblem::ApiKey));
}

#[test]
fn a_missing_directory_is_reported_before_the_value() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    write_mappings(&fx, &[("~/gone", "nobody")]);

    let got = judged(&fx);
    assert_eq!(got[0].dir, MappedDir::Missing(fx.env.home.join("gone")));
    assert_eq!(got[0].problem, Some(MappingProblem::MissingDirectory));
}

#[test]
fn a_key_naming_a_file_is_a_missing_directory() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.engine.set_alias(&a, Some("work")).unwrap();
    fs::write(fx.env.home.join("f"), "").unwrap();
    write_mappings(&fx, &[("~/f", "work")]);

    let got = judged(&fx);
    assert_eq!(got[0].dir, MappedDir::Missing(fx.env.home.join("f")));
    assert_eq!(got[0].problem, Some(MappingProblem::MissingDirectory));
}

#[test]
fn equivalent_entries_conflict_only_when_their_values_differ() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.engine.set_alias(&a, Some("work")).unwrap();
    let w = mkdir(&fx, "w");
    let absolute = w.to_str().unwrap();

    write_mappings(&fx, &[("~/w", "work"), (absolute, "b@x.co")]);
    let got = judged(&fx);
    assert_eq!(got.len(), 2);
    assert!(
        got.iter()
            .all(|m| m.problem == Some(MappingProblem::Conflict))
    );

    write_mappings(&fx, &[("~/w", "work"), (absolute, "work")]);
    let got = judged(&fx);
    assert_eq!(got.len(), 2);
    assert!(got.iter().all(|m| m.problem.is_none()));
}

#[test]
fn conflict_takes_precedence_over_the_value_s_problem() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.engine.set_alias(&a, Some("work")).unwrap();
    let w = mkdir(&fx, "w");
    write_mappings(&fx, &[("~/w", "2"), (w.to_str().unwrap(), "work")]);

    let got = judged(&fx);
    assert_eq!(got.len(), 2);
    assert!(
        got.iter()
            .all(|m| m.problem == Some(MappingProblem::Conflict)),
        "{:?}",
        got.iter().map(|m| m.problem).collect::<Vec<_>>()
    );
}

#[test]
fn with_no_store_nothing_resolves_and_nothing_is_created() {
    let fx = Fx::new();
    mkdir(&fx, "w");
    write_mappings(&fx, &[("~/w", "work")]);

    let got = judged(&fx);
    assert_eq!(got[0].account, None);
    assert_eq!(got[0].problem, Some(MappingProblem::NoAccount));
    assert!(!fx.env.data_dir().exists());
}

#[test]
fn an_entry_keeps_what_its_value_names_whatever_its_directory_or_conflicts() {
    let fx = Fx::new();
    add_same_email(&fx, "a@x.co");
    let one = AccountId::from_string("same-1");
    fx.engine.set_alias(&one, Some("work")).unwrap();
    let w = mkdir(&fx, "w");
    write_mappings(
        &fx,
        &[
            ("~/gone", "a@x.co"),
            ("~/w", "work"),
            (w.to_str().unwrap(), "2"),
        ],
    );

    let got = judged(&fx);
    assert_eq!(got[0].problem, Some(MappingProblem::MissingDirectory));
    assert_eq!(
        got[0].named,
        Named::DoesNot {
            account: None,
            problem: MappingProblem::Ambiguous
        }
    );
    assert_eq!(got[1].problem, Some(MappingProblem::Conflict));
    assert!(
        matches!(&got[1].named, Named::Applies(row) if row.id == one),
        "{:?}",
        got[1].named
    );
    assert_eq!(got[2].problem, Some(MappingProblem::Conflict));
    assert_eq!(
        got[2].named,
        Named::DoesNot {
            account: None,
            problem: MappingProblem::Position
        }
    );
}
