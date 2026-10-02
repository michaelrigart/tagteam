//! §12.2's link sync: what a profile shares with the outer home, and what it never touches
//! (Review Focus 5). Profiles live under the fixture's data dir. The source home is the
//! fixture's `~/.claude`, unless a marker says otherwise.
mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use common::{FakeFx, Fx};
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::Engine;
use tagteam_engine::profiles::SyncReport;
use tagteam_engine::settings::Settings;
use tagteam_fake::FAKE_AGENT;
use tagteam_provider::profile::{LINKS_FILE, LinksRecord, MARKER_FILE, ProfileMarker};
use tagteam_provider::{Provider, Read};

/// The fixture's `~/.claude` entries that Claude Code's allowlist names, in the policy's
/// order: the must-share ones first.
const FIXTURE_SHARED: [&str; 6] = [
    "projects",
    "history.jsonl",
    "CLAUDE.md",
    "settings.json",
    "skills",
    "plugins",
];

/// A profile as a first launch leaves it before its sync: the marker, and nothing else.
fn setup(fx: &Fx) -> PathBuf {
    fx.make_profile_for(fx.cc.as_ref(), &AccountId::from_string("0192-a"))
}

/// The source home Claude Code's policy names for the fixture.
fn source(fx: &Fx) -> PathBuf {
    fx.cc.share_policy(&fx.env).source
}

/// A launch's sync.
fn sync(fx: &Fx, profile: &Path) -> SyncReport {
    fx.engine
        .sync_profile_links(fx.cc.as_ref(), profile, false)
        .unwrap()
}

/// The sync of a launch that joins a running session.
fn join(fx: &Fx, profile: &Path) -> SyncReport {
    fx.engine
        .sync_profile_links(fx.cc.as_ref(), profile, true)
        .unwrap()
}

/// An engine over the fixture whose settings also share `names` (`run.share_extra`).
fn sharing(fx: &Fx, names: &[&str]) -> Engine {
    fx.engine_with_settings(Settings {
        share_extra: names.iter().map(|n| n.to_string()).collect(),
        ..Settings::default()
    })
}

/// `path`'s link target, when it is a link.
fn link(path: &Path) -> Option<PathBuf> {
    let meta = fs::symlink_metadata(path).ok()?;
    meta.file_type()
        .is_symlink()
        .then(|| fs::read_link(path).unwrap())
}

fn resolved(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap()
}

/// Nothing at all at `path`, not even a link.
fn absent(path: &Path) -> bool {
    fs::symlink_metadata(path).is_err()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn record(profile: &Path) -> LinksRecord {
    match LinksRecord::read(profile) {
        Read::Present(r) => r,
        other => panic!("the links record should read: {other:?}"),
    }
}

/// The names in `dir`, sorted.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// What `path` is, to compare before and after: a link's target, a directory's names, or a
/// file's text.
fn describe(path: &Path) -> String {
    let meta = fs::symlink_metadata(path).unwrap();
    if meta.file_type().is_symlink() {
        format!("link {}", fs::read_link(path).unwrap().display())
    } else if meta.is_dir() {
        format!("dir {:?}", entries(path))
    } else {
        format!("file {:?}", fs::read_to_string(path).unwrap())
    }
}

/// Moves the source's `name` into `~/dotfiles` and links it back, as GNU stow does.
fn stow(fx: &Fx, name: &str) -> PathBuf {
    let dot = fx.env.home.join("dotfiles");
    fs::create_dir_all(&dot).unwrap();
    let moved = dot.join(name);
    fs::rename(source(fx).join(name), &moved).unwrap();
    symlink(&moved, source(fx).join(name)).unwrap();
    moved
}

/// Rewrites the profile's marker so that its outer home is `home`, as a launch with
/// `CLAUDE_CONFIG_DIR=<home>` records it.
fn point_marker_at(fx: &Fx, profile: &Path, home: &Path) {
    let mut env = fx.env.clone();
    env.claude_config_dir = Some(home.as_os_str().to_owned());
    let Read::Present(marker) = ProfileMarker::read(profile) else {
        panic!("the marker reads");
    };
    ProfileMarker {
        outer: fx.cc.outer_home(&env),
        ..marker
    }
    .write(profile)
    .unwrap();
}

#[test]
fn every_allowlisted_entry_of_the_source_home_is_linked_to_its_fully_resolved_path() {
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    // A stow-style CLAUDE.md: the link must reach the file itself, in one hop (Appendix A.1).
    let moved = stow(&fx, "CLAUDE.md");

    let report = sync(&fx, &profile);

    assert_eq!(report.created, FIXTURE_SHARED);
    assert!(
        report.removed.is_empty() && report.warnings.is_empty(),
        "{report:?}"
    );
    for name in FIXTURE_SHARED {
        assert_eq!(
            link(&profile.join(name)),
            Some(resolved(&src.join(name))),
            "{name}"
        );
    }
    assert_eq!(
        link(&profile.join("CLAUDE.md")),
        Some(resolved(&moved)),
        "the fully resolved source, not the stow link"
    );
    for name in ["keybindings.json", "agents", "commands"] {
        assert!(
            absent(&profile.join(name)),
            "{name} is not in the source home, so nothing links it"
        );
    }
    let want: BTreeMap<String, PathBuf> = FIXTURE_SHARED
        .iter()
        .map(|n| (n.to_string(), resolved(&src.join(n))))
        .collect();
    assert_eq!(
        record(&profile).links,
        want,
        "the record names every link tagteam made"
    );
    assert_eq!(mode(&profile.join(LINKS_FILE)), 0o600);
    assert_eq!(
        fs::read_to_string(src.join("history.jsonl")).unwrap(),
        "{\"display\":\"hi\"}\n",
        "an existing must-share entry is never written"
    );
    assert_eq!(
        sync(&fx, &profile),
        SyncReport::default(),
        "a second launch changes nothing"
    );
}

#[test]
fn a_missing_must_share_entry_is_created_empty_in_the_source_home_and_linked() {
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    fs::remove_dir_all(src.join("projects")).unwrap();
    fs::remove_file(src.join("history.jsonl")).unwrap();

    let report = sync(&fx, &profile);

    assert!(src.join("projects").is_dir());
    assert!(entries(&src.join("projects")).is_empty());
    assert_eq!(mode(&src.join("projects")), 0o700);
    assert_eq!(fs::read(src.join("history.jsonl")).unwrap(), b"");
    assert_eq!(mode(&src.join("history.jsonl")), 0o600);
    for name in ["projects", "history.jsonl"] {
        assert_eq!(
            link(&profile.join(name)),
            Some(resolved(&src.join(name))),
            "{name}"
        );
    }
    assert_eq!(report.created[..2], ["projects", "history.jsonl"]);
}

#[test]
fn a_source_home_that_does_not_exist_yet_is_created_with_its_must_share_entries() {
    let fx = Fx::new();
    let profile = setup(&fx);
    let fresh = fx.env.home.join("fresh-claude");
    point_marker_at(&fx, &profile, &fresh);

    let report = sync(&fx, &profile);

    assert_eq!(report.created, ["projects", "history.jsonl"]);
    assert_eq!(mode(&fresh), 0o700);
    assert!(fresh.join("projects").is_dir());
    assert_eq!(fs::read(fresh.join("history.jsonl")).unwrap(), b"");
}

#[test]
fn private_entries_and_tagteam_s_own_files_are_never_linked_even_through_share_extra() {
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    let dirs = ["sessions", "cache", "daemon", ".oauth_refresh.lock"];
    let files = [
        ".credentials.json",
        "settings.local.json",
        "daemon.json",
        "policy-limits.json.signature",
        ".tagteam-x",
    ];
    for dir in dirs {
        fs::create_dir_all(src.join(dir)).unwrap();
    }
    for file in files {
        fs::write(src.join(file), "x").unwrap();
    }
    let engine = sharing(&fx, &["cache", "settings.local.json", ".tagteam-x"]);

    let report = engine
        .sync_profile_links(fx.cc.as_ref(), &profile, false)
        .unwrap();

    for name in dirs.iter().chain(files.iter()) {
        assert!(absent(&profile.join(name)), "{name} was linked");
        assert!(!report.created.iter().any(|n| n == name), "{name}");
    }
    assert_eq!(
        report.warnings.len(),
        3,
        "one for each private name in run.share_extra, and no unknown-entry notice: {:?}",
        report.warnings
    );
    for name in ["cache", "settings.local.json", ".tagteam-x"] {
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("run.share_extra") && w.contains(name)),
            "{name}: {:?}",
            report.warnings
        );
    }
}

#[test]
fn run_share_extra_shares_a_user_s_own_entry() {
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    fs::create_dir_all(src.join("hook-data")).unwrap();

    let report = sharing(&fx, &["hook-data"])
        .sync_profile_links(fx.cc.as_ref(), &profile, false)
        .unwrap();

    assert!(
        report.created.iter().any(|n| n == "hook-data"),
        "{report:?}"
    );
    assert_eq!(
        link(&profile.join("hook-data")),
        Some(resolved(&src.join("hook-data")))
    );
    assert!(
        report.warnings.is_empty(),
        "a shared entry is not unknown: {:?}",
        report.warnings
    );
}

#[test]
fn an_unknown_entry_of_the_source_home_stays_private_and_is_noted_once() {
    // Review Focus 5: a new CC feature directory tagteam does not know.
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    fs::create_dir_all(src.join("newfeature")).unwrap();
    fs::create_dir_all(src.join("sessions")).unwrap(); // known-private: never noted

    let first = sync(&fx, &profile);

    assert_eq!(first.warnings.len(), 1, "{:?}", first.warnings);
    assert!(
        first.warnings[0].contains(&src.join("newfeature").display().to_string()),
        "{:?}",
        first.warnings
    );
    assert!(
        absent(&profile.join("newfeature")),
        "it stays private: no link"
    );
    assert!(absent(&profile.join("sessions")));
    assert_eq!(
        record(&profile).noted_unknown,
        BTreeSet::from(["newfeature".to_owned()])
    );

    let second = sync(&fx, &profile);
    assert!(
        second.warnings.is_empty(),
        "noted once: {:?}",
        second.warnings
    );
    assert!(absent(&profile.join("newfeature")));
}

#[test]
fn a_shared_file_the_profile_holds_as_a_regular_file_warns_naming_both_and_is_left_alone() {
    // Review Focus 5: something replaced the profile's settings.json link with a real file.
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    sync(&fx, &profile);
    let copy = profile.join("settings.json");
    fs::remove_file(&copy).unwrap();
    fs::write(&copy, "{\"theme\":\"light\"}\n").unwrap();

    let report = sync(&fx, &profile);

    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    let w = &report.warnings[0];
    assert!(
        w.contains(&copy.display().to_string())
            && w.contains(&src.join("settings.json").display().to_string()),
        "{w}"
    );
    assert_eq!(
        fs::read_to_string(&copy).unwrap(),
        "{\"theme\":\"light\"}\n",
        "the profile's copy is untouched"
    );
    assert_eq!(
        fs::read_to_string(src.join("settings.json")).unwrap(),
        "{\"theme\":\"dark\"}\n",
        "and so is the shared one"
    );
    assert!(
        report.created.is_empty() && report.removed.is_empty(),
        "{report:?}"
    );
    assert!(
        !record(&profile).links.contains_key("settings.json"),
        "no longer tagteam's link"
    );
    assert!(
        link(&profile.join("CLAUDE.md")).is_some(),
        "the other links stay"
    );
    assert_eq!(
        sync(&fx, &profile).warnings.len(),
        1,
        "a split file is reported at every launch"
    );
}

#[test]
fn a_real_directory_where_a_shared_link_belongs_warns_and_is_left_alone() {
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    fs::create_dir_all(profile.join("skills/mine")).unwrap();
    fs::write(profile.join("skills/mine/SKILL.md"), "mine\n").unwrap();

    let report = sync(&fx, &profile);

    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    let w = &report.warnings[0];
    assert!(
        w.contains(&profile.join("skills").display().to_string())
            && w.contains(&src.join("skills").display().to_string()),
        "{w}"
    );
    assert_eq!(
        fs::read_to_string(profile.join("skills/mine/SKILL.md")).unwrap(),
        "mine\n"
    );
    assert!(link(&profile.join("skills")).is_none());
    assert!(!report.created.iter().any(|n| n == "skills"));
    assert!(
        src.join("skills/s/SKILL.md").is_file(),
        "the shared one is untouched"
    );
}

/// Plants `name` in a fresh profile with `plant`. The sync then refuses with
/// `profile-split`, naming both paths and saying `why`, for a launch and for a join, and
/// writes nothing anywhere.
fn assert_split(name: &str, why: &str, plant: impl Fn(&Fx, &Path)) {
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    plant(&fx, &profile.join(name));
    // The other must-share entry is missing, so a sync that went ahead would create it.
    let other = src.join(if name == "projects" {
        "history.jsonl"
    } else {
        "projects"
    });
    if other.is_dir() {
        fs::remove_dir_all(&other).unwrap();
    } else {
        fs::remove_file(&other).unwrap();
    }
    let (copy, shared) = (describe(&profile.join(name)), describe(&src.join(name)));

    for joining in [false, true] {
        let err = fx
            .engine
            .sync_profile_links(fx.cc.as_ref(), &profile, joining)
            .unwrap_err();
        assert_eq!(err.kind(), "profile-split", "{name}: {err}");
        let text = err.to_string();
        assert!(
            text.contains(&profile.join(name).display().to_string())
                && text.contains(&src.join(name).display().to_string()),
            "{text}"
        );
        assert!(text.contains(why), "{name}: {text}");
    }
    assert_eq!(
        describe(&profile.join(name)),
        copy,
        "the profile's copy is untouched"
    );
    assert_eq!(
        describe(&src.join(name)),
        shared,
        "and so is the shared one"
    );
    assert!(absent(&other), "nothing was created in the source home");
    let mut want = vec![MARKER_FILE.to_owned(), name.to_owned()];
    want.sort();
    assert_eq!(
        entries(&profile),
        want,
        "no link was made and no record written"
    );
}

#[test]
fn a_real_history_file_in_the_profile_refuses_with_profile_split_and_changes_nothing() {
    // Review Focus 5.
    assert_split("history.jsonl", "is a real copy", |_, p| {
        fs::write(p, "private history\n").unwrap()
    });
}

#[test]
fn a_real_projects_directory_in_the_profile_refuses_with_profile_split_and_changes_nothing() {
    assert_split("projects", "is a real copy", |_, p| {
        fs::create_dir_all(p.join("-x/memory")).unwrap();
        fs::write(p.join("-x/memory/MEMORY.md"), "private memory\n").unwrap();
    });
}

#[test]
fn a_must_share_link_that_resolves_elsewhere_refuses_as_a_split_too() {
    assert_split("history.jsonl", "links somewhere other than", |fx, p| {
        let elsewhere = fx.env.home.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("history.jsonl"), "elsewhere\n").unwrap();
        symlink(elsewhere.join("history.jsonl"), p).unwrap();
    });
}

#[test]
fn a_link_tagteam_did_not_make_is_never_touched() {
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    let elsewhere = fx.env.home.join("elsewhere");
    for dir in ["skills", "notes"] {
        fs::create_dir_all(elsewhere.join(dir)).unwrap();
    }
    symlink(elsewhere.join("skills"), profile.join("skills")).unwrap(); // where tagteam's goes
    symlink(elsewhere.join("notes"), profile.join("notes")).unwrap(); // on no list
    symlink(src.join("plugins"), profile.join("plugins")).unwrap(); // the share, unresolved

    let report = sync(&fx, &profile);

    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    let w = &report.warnings[0];
    assert!(
        w.contains(&profile.join("skills").display().to_string())
            && w.contains(&src.join("skills").display().to_string()),
        "{w}"
    );
    assert_eq!(
        link(&profile.join("skills")),
        Some(elsewhere.join("skills"))
    );
    assert_eq!(link(&profile.join("notes")), Some(elsewhere.join("notes")));
    assert_eq!(
        link(&profile.join("plugins")),
        Some(src.join("plugins")),
        "a link that already reaches the share is left as it is"
    );
    let r = record(&profile);
    for name in ["skills", "notes", "plugins"] {
        assert!(!r.links.contains_key(name), "{name} is not tagteam's");
    }

    // tagteam's own link, repointed by someone since, is no longer tagteam's: even with its
    // source gone, it is never removed.
    fs::remove_file(profile.join("CLAUDE.md")).unwrap();
    fs::write(elsewhere.join("CLAUDE.md"), "mine\n").unwrap();
    symlink(elsewhere.join("CLAUDE.md"), profile.join("CLAUDE.md")).unwrap();
    fs::remove_file(src.join("CLAUDE.md")).unwrap();
    let again = sync(&fx, &profile);
    assert!(again.removed.is_empty(), "{again:?}");
    assert_eq!(
        link(&profile.join("CLAUDE.md")),
        Some(elsewhere.join("CLAUDE.md"))
    );
    assert!(!record(&profile).links.contains_key("CLAUDE.md"));
}

#[test]
fn a_link_whose_source_disappeared_is_removed() {
    let fx = Fx::new();
    let profile = setup(&fx);
    sync(&fx, &profile);
    fs::remove_dir_all(source(&fx).join("skills")).unwrap();

    let report = sync(&fx, &profile);

    assert_eq!(report.removed, ["skills"]);
    assert!(report.created.is_empty(), "{report:?}");
    assert!(absent(&profile.join("skills")));
    assert!(!record(&profile).links.contains_key("skills"));
}

#[test]
fn a_link_whose_entry_left_the_allowlist_is_removed_and_its_source_kept() {
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    for dir in ["hook-data", "tool-cache"] {
        fs::create_dir_all(src.join(dir)).unwrap();
    }
    sharing(&fx, &["hook-data", "tool-cache"])
        .sync_profile_links(fx.cc.as_ref(), &profile, false)
        .unwrap();
    // One of the two links was replaced by a real directory meanwhile.
    fs::remove_file(profile.join("tool-cache")).unwrap();
    fs::create_dir(profile.join("tool-cache")).unwrap();

    let report = sync(&fx, &profile); // run.share_extra names neither any more

    assert_eq!(report.removed, ["hook-data"]);
    assert!(absent(&profile.join("hook-data")));
    assert!(
        src.join("hook-data").is_dir(),
        "the source is never touched"
    );
    assert!(
        profile.join("tool-cache").is_dir() && link(&profile.join("tool-cache")).is_none(),
        "a real directory is never removed"
    );
    let r = record(&profile);
    assert!(!r.links.contains_key("hook-data") && !r.links.contains_key("tool-cache"));
}

#[test]
fn a_link_to_a_moved_source_is_made_again() {
    let fx = Fx::new();
    let profile = setup(&fx);
    sync(&fx, &profile);
    let moved = stow(&fx, "CLAUDE.md");

    let report = sync(&fx, &profile);

    assert_eq!(report.removed, ["CLAUDE.md"]);
    assert_eq!(report.created, ["CLAUDE.md"]);
    assert_eq!(link(&profile.join("CLAUDE.md")), Some(resolved(&moved)));
    assert_eq!(record(&profile).links["CLAUDE.md"], resolved(&moved));
}

#[test]
fn links_follow_the_outer_home_the_marker_records() {
    let fx = Fx::new();
    let profile = setup(&fx);
    sync(&fx, &profile);
    let other = fx.env.home.join("other-claude");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("CLAUDE.md"), "other\n").unwrap();
    point_marker_at(&fx, &profile, &other);

    let report = sync(&fx, &profile);

    assert_eq!(report.removed, FIXTURE_SHARED);
    assert_eq!(report.created, ["projects", "history.jsonl", "CLAUDE.md"]);
    assert_eq!(
        link(&profile.join("CLAUDE.md")),
        Some(resolved(&other.join("CLAUDE.md")))
    );
    assert!(
        other.join("projects").is_dir(),
        "the new home's must-share entries"
    );
    assert_eq!(fs::read(other.join("history.jsonl")).unwrap(), b"");
    for name in ["settings.json", "skills", "plugins"] {
        assert!(absent(&profile.join(name)), "{name}: the new home has none");
    }
    assert!(
        source(&fx)
            .join("projects/-work-app/memory/MEMORY.md")
            .is_file(),
        "the old home is untouched"
    );
}

#[test]
fn a_joining_sync_only_creates_missing_links() {
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    sync(&fx, &profile);
    let (old_skills, old_claude) = (
        link(&profile.join("skills")),
        link(&profile.join("CLAUDE.md")),
    );
    fs::remove_dir_all(src.join("skills")).unwrap();
    stow(&fx, "CLAUDE.md");
    fs::create_dir_all(src.join("agents")).unwrap();

    let report = join(&fx, &profile);

    assert_eq!(report.created, ["agents"]);
    assert!(report.removed.is_empty(), "{report:?}");
    assert_eq!(
        link(&profile.join("skills")),
        old_skills,
        "a running session's links never change"
    );
    assert_eq!(link(&profile.join("CLAUDE.md")), old_claude);

    // The next launch that does not join tidies up.
    let report = sync(&fx, &profile);
    assert_eq!(report.removed, ["CLAUDE.md", "skills"]);
    assert_eq!(report.created, ["CLAUDE.md"]);
}

#[test]
fn a_join_refuses_a_must_share_link_that_no_longer_resolves_where_the_source_does() {
    // The outer home's projects/ moved behind a link while a session runs: the profile's link
    // still points at the old place, and a join cannot make it again under the session. The
    // session and the outer home would use different memory, so the join refuses before
    // writing anything; a quiescent launch makes the link again.
    let fx = Fx::new();
    let profile = setup(&fx);
    sync(&fx, &profile);
    let old = link(&profile.join("projects"));
    let moved = stow(&fx, "projects");
    let before = record(&profile);

    let err = fx
        .engine
        .sync_profile_links(fx.cc.as_ref(), &profile, true)
        .unwrap_err();

    assert_eq!(err.kind(), "profile-split", "{err}");
    // Nothing is to merge: the fix is to let the session end, then launch again.
    let text = err.to_string();
    assert!(text.contains("end that session"), "{text}");
    assert!(!text.contains("merge"), "{text}");
    assert_eq!(link(&profile.join("projects")), old, "nothing was written");
    assert_eq!(record(&profile), before);

    let report = sync(&fx, &profile);
    assert_eq!(report.removed, ["projects"]);
    assert_eq!(report.created, ["projects"]);
    assert_eq!(link(&profile.join("projects")), Some(resolved(&moved)));
}

#[test]
fn a_join_recreates_a_missing_must_share_source_its_link_points_at() {
    // history.jsonl vanished from the outer home while a session runs. Its link still names
    // the place step 2 creates it again, so the join recreates it empty and keeps the link.
    let fx = Fx::new();
    let profile = setup(&fx);
    sync(&fx, &profile);
    let src = source(&fx);
    let old = link(&profile.join("history.jsonl"));
    fs::remove_file(src.join("history.jsonl")).unwrap();

    let report = join(&fx, &profile);

    assert_eq!(fs::read(src.join("history.jsonl")).unwrap(), b"");
    assert_eq!(link(&profile.join("history.jsonl")), old);
    assert!(report.removed.is_empty(), "{report:?}");
}

#[test]
fn an_optional_source_that_does_not_resolve_is_not_linked() {
    // A link to nothing, one that loops, and one through a file: none names an entry to share.
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    fs::remove_file(src.join("CLAUDE.md")).unwrap();
    symlink(src.join("CLAUDE.md"), src.join("CLAUDE.md")).unwrap();
    symlink(src.join("missing"), src.join("keybindings.json")).unwrap();
    symlink(src.join("CLAUDE.md/inside"), src.join("themes")).unwrap();
    fs::remove_file(src.join("settings.json")).unwrap();
    symlink(src.join("history.jsonl/inside"), src.join("settings.json")).unwrap();

    let report = sync(&fx, &profile);

    for name in ["CLAUDE.md", "keybindings.json", "themes", "settings.json"] {
        assert!(absent(&profile.join(name)), "{name} is not linked");
        assert!(link(&src.join(name)).is_some(), "{name} is left as it is");
    }
    for name in ["projects", "history.jsonl", "skills", "plugins"] {
        assert_eq!(
            link(&profile.join(name)),
            Some(resolved(&src.join(name))),
            "{name}"
        );
    }
    assert_eq!(
        report.created,
        ["projects", "history.jsonl", "skills", "plugins"]
    );
}

#[test]
fn a_must_share_source_that_does_not_resolve_refuses_as_a_link_to_nothing() {
    for (what, target) in [("dangling", "nowhere"), ("a self-loop", "history.jsonl")] {
        let fx = Fx::new();
        let profile = setup(&fx);
        let src = source(&fx);
        fs::remove_file(src.join("history.jsonl")).unwrap();
        symlink(src.join(target), src.join("history.jsonl")).unwrap();

        let err = fx
            .engine
            .sync_profile_links(fx.cc.as_ref(), &profile, false)
            .unwrap_err();

        assert_eq!(err.kind(), "invalid-input", "{what}: {err}");
        assert!(err.to_string().contains("link to nothing"), "{what}: {err}");
        assert_eq!(
            entries(&profile),
            [MARKER_FILE],
            "{what}: nothing was linked"
        );
    }
}

#[test]
fn a_source_home_that_is_not_a_directory_refuses_before_anything_is_created() {
    // The outer home is a link to nothing, one that loops, or a regular file: there is no
    // directory to create the must-share entries in, nor any entry to share.
    let fx = Fx::new();
    let elsewhere = fx.env.home.join("not-claude");
    type Make = fn(&Path);
    let cases: [(&str, Make); 3] = [
        ("a link to nothing", |home| {
            symlink(home.with_file_name("nowhere"), home).unwrap()
        }),
        ("a self-loop", |home| symlink(home, home).unwrap()),
        ("a regular file", |home| fs::write(home, "x").unwrap()),
    ];
    for (what, make) in cases {
        let profile = setup(&fx);
        let home = elsewhere.join(what.replace(' ', "-"));
        fs::create_dir_all(&elsewhere).unwrap();
        make(&home);
        point_marker_at(&fx, &profile, &home);
        let before = describe(&home);

        let err = fx
            .engine
            .sync_profile_links(fx.cc.as_ref(), &profile, false)
            .unwrap_err();

        assert_eq!(err.kind(), "invalid-input", "{what}: {err}");
        assert!(
            err.to_string().contains(&home.display().to_string()),
            "{what}: {err}"
        );
        assert_eq!(
            describe(&home),
            before,
            "{what}: the outer home is untouched"
        );
        assert_eq!(
            entries(&profile),
            [MARKER_FILE],
            "{what}: nothing was linked"
        );
        fs::remove_dir_all(&profile).unwrap();
    }
}

#[test]
fn a_profile_whose_marker_or_record_cannot_be_read_is_not_synced() {
    // No marker: the outer home is unknown.
    let fx = Fx::new();
    let profile = setup(&fx);
    fs::remove_file(profile.join(MARKER_FILE)).unwrap();
    let err = fx
        .engine
        .sync_profile_links(fx.cc.as_ref(), &profile, false)
        .unwrap_err();
    assert_eq!(err.kind(), "invalid-input", "{err}");
    assert!(entries(&profile).is_empty(), "nothing was linked");

    // A corrupt marker or record is unreadable, never taken for absent (§4.3).
    for file in [MARKER_FILE, LINKS_FILE] {
        let fx = Fx::new();
        let profile = setup(&fx);
        fs::write(profile.join(file), "{").unwrap();
        let err = fx
            .engine
            .sync_profile_links(fx.cc.as_ref(), &profile, false)
            .unwrap_err();
        assert_eq!(err.kind(), "unreadable", "{file}: {err}");
        assert!(
            FIXTURE_SHARED.iter().all(|n| absent(&profile.join(n))),
            "{file}: nothing was linked"
        );
        assert_eq!(fs::read_to_string(profile.join(file)).unwrap(), "{");
    }

    // Another provider's profile.
    let fx = Fx::new();
    let profile = setup(&fx);
    let Read::Present(marker) = ProfileMarker::read(&profile) else {
        panic!("the marker reads");
    };
    ProfileMarker {
        provider: ProviderId::new(FAKE_AGENT),
        ..marker
    }
    .write(&profile)
    .unwrap();
    let err = fx
        .engine
        .sync_profile_links(fx.cc.as_ref(), &profile, false)
        .unwrap_err();
    assert_eq!(err.kind(), "invalid-input", "{err}");
}

#[test]
fn fake_agent_profiles_share_by_fake_agent_s_own_policy() {
    // §12.1: the sharing rules are engine-generic; FakeAgent's lists are deliberately unlike
    // Claude Code's (Decision 14).
    let ffx = FakeFx::new();
    let alice = ffx.fake_add("alice", "tok-a", "renew-a");
    let home = ffx.fake.share_policy(&ffx.fx.env).source;
    fs::create_dir_all(home.join("notes")).unwrap();
    fs::write(home.join("prefs.json"), "{}\n").unwrap();
    fs::create_dir_all(home.join("widgets")).unwrap();
    let profile = ffx.fx.make_profile_for(ffx.fake.as_ref(), &alice);

    let report = ffx
        .engine
        .sync_profile_links(ffx.fake.as_ref(), &profile, false)
        .unwrap();

    assert_eq!(report.created, ["journal.log", "notes", "prefs.json"]);
    assert_eq!(
        fs::read(home.join("journal.log")).unwrap(),
        b"",
        "its must-share entry, created empty"
    );
    for name in ["journal.log", "notes", "prefs.json"] {
        assert_eq!(
            link(&profile.join(name)),
            Some(resolved(&home.join(name))),
            "{name}"
        );
    }
    for name in ["credential.json", "identity.json", "widgets"] {
        assert!(absent(&profile.join(name)), "{name} stays private");
    }
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].contains(&home.join("widgets").display().to_string()),
        "{:?}",
        report.warnings
    );
}
