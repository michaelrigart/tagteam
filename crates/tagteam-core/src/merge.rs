//! §12.4's merge-back as a pure function: the profile's changes since the seed's baseline,
//! applied over the default file key by key, at §12.4's granularity (`projects.<path>.<key>`
//! and `mcpServers.<name>`). The default file wins a key both sides changed. A provider
//! applies the result with the §9.5 splice of the whole subtrees (Decision 4), so this module
//! never sees bytes.

use serde_json::{Map, Value};

/// One key of §12.4's diff: `projects.<path>.<key>` or `mcpServers.<name>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum MergeKey {
    Project { path: String, key: String },
    McpServer { name: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct MergeResult {
    /// The new `projects` and `mcpServers` values for the default file (`None`: leave the
    /// key as it is, because nothing changed under it).
    pub projects: Option<serde_json::Value>,
    pub mcp_servers: Option<serde_json::Value>,
    /// Keys the profile changed (or removed) that were applied.
    pub applied: Vec<MergeKey>,
    /// Keys both sides changed since the baseline: the default's value was kept (§12.4 step 3).
    pub conflicts: Vec<MergeKey>,
}

/// §12.4: the profile's changes since `baseline`, applied over `default`. Each argument is
/// the pair (`projects`, `mcpServers`) as JSON values, `Value::Null` when absent.
///
/// - A key the profile changed, added or removed is applied only where the default still holds
///   the baseline's value. Where the default changed it too, that is a conflict and the
///   default's value stays; where both made the same change, there is nothing to do.
/// - A project the profile removed loses each of its keys, and goes once none of the default's
///   remains. A project it added is appended with its keys.
/// - A subtree the profile does not hold as an object changed nothing: Claude Code never drops
///   `projects` or `mcpServers`, so their absence means a reset file, not that all went.
/// - A default value that is there but not an object takes no change: each change under it is
///   a conflict.
///
/// `applied` and `conflicts` are sorted.
pub fn three_way(
    baseline: (&serde_json::Value, &serde_json::Value),
    profile: (&serde_json::Value, &serde_json::Value),
    default: (&serde_json::Value, &serde_json::Value),
) -> MergeResult {
    let mut keys = Keys::default();
    let projects = merge_projects(baseline.0, profile.0, default.0, &mut keys).map(Value::Object);
    let mcp_servers = profile
        .1
        .as_object()
        .and_then(|mine| {
            merge_level(
                baseline.1.as_object(),
                Some(mine),
                // At the top level `null` means the key is absent (the contract).
                (!default.1.is_null()).then_some(default.1),
                &|name: &str| MergeKey::McpServer {
                    name: name.to_owned(),
                },
                &mut keys,
            )
        })
        .map(Value::Object);
    keys.applied.sort();
    keys.conflicts.sort();
    MergeResult {
        projects,
        mcp_servers,
        applied: keys.applied,
        conflicts: keys.conflicts,
    }
}

type Object = Map<String, Value>;

/// What the merge reports, in the order it meets the keys (sorted at the end).
#[derive(Default)]
struct Keys {
    applied: Vec<MergeKey>,
    conflicts: Vec<MergeKey>,
}

fn get<'a>(level: Option<&'a Object>, name: &str) -> Option<&'a Value> {
    level.and_then(|o| o.get(name))
}

/// The names whose value differs between `base` and `mine`: `mine`'s in its order, then those
/// only `base` holds, in its.
fn changed<'a>(base: Option<&'a Object>, mine: Option<&'a Object>) -> Vec<&'a str> {
    let only_base = base
        .into_iter()
        .flat_map(|o| o.keys())
        .filter(move |k| get(mine, k).is_none());
    mine.into_iter()
        .flat_map(|o| o.keys())
        .chain(only_base)
        .map(String::as_str)
        .filter(|k| get(base, k) != get(mine, k))
        .collect()
}

/// One level of the merge: the profile's change from `base` to `mine` (`None`: the profile
/// holds no such object, so each key it had went), applied over `theirs`, the default's value
/// at this level. `None` means absent, so the level starts empty. Any value that is not an
/// object, `null` included, is present and takes no change: its keys are conflicts. Returns
/// the level's new object, or `None` when nothing was applied.
fn merge_level(
    base: Option<&Object>,
    mine: Option<&Object>,
    theirs: Option<&Value>,
    key: &dyn Fn(&str) -> MergeKey,
    keys: &mut Keys,
) -> Option<Object> {
    let changed = changed(base, mine);
    if changed.is_empty() {
        return None;
    }
    let mut out = match theirs {
        None => Object::new(),
        Some(Value::Object(o)) => o.clone(),
        Some(_) => {
            keys.conflicts.extend(changed.into_iter().map(key));
            return None;
        }
    };
    let mut any = false;
    for name in changed {
        let (was, now) = (get(base, name), get(mine, name));
        let held = out.get(name);
        if held == now {
            // Both sides made this change.
            continue;
        }
        if held != was {
            keys.conflicts.push(key(name));
            continue;
        }
        match now {
            Some(v) => {
                out.insert(name.to_owned(), v.clone());
            }
            None => {
                out.shift_remove(name);
            }
        }
        keys.applied.push(key(name));
        any = true;
    }
    any.then_some(out)
}

/// `projects`, two levels deep: each project's keys through `merge_level`, then the project
/// itself set, appended, or removed when the profile removed it and none of the default's keys
/// remain.
fn merge_projects(base: &Value, mine: &Value, theirs: &Value, keys: &mut Keys) -> Option<Object> {
    let mine = mine.as_object()?;
    let base = base.as_object();
    let mut projects = match theirs {
        Value::Null => Some(Object::new()),
        Value::Object(o) => Some(o.clone()),
        _ => None,
    };
    let paths: Vec<&str> = mine
        .keys()
        .chain(
            base.into_iter()
                .flat_map(|o| o.keys())
                .filter(|p| !mine.contains_key(p.as_str())),
        )
        .map(String::as_str)
        .collect();
    let mut any = false;
    for path in paths {
        let was = get(base, path).and_then(Value::as_object);
        let now = match mine.get(path) {
            None => None,
            Some(Value::Object(o)) => Some(o),
            // A project that is not an object has no keys to read: it changed nothing.
            Some(_) => continue,
        };
        let key = |k: &str| MergeKey::Project {
            path: path.to_owned(),
            key: k.to_owned(),
        };
        let Some(map) = projects.as_mut() else {
            // The default's `projects` is not an object: it takes no change.
            keys.conflicts
                .extend(changed(was, now).into_iter().map(key));
            continue;
        };
        let Some(project) = merge_level(was, now, map.get(path), &key, keys) else {
            continue;
        };
        if now.is_none() && project.is_empty() {
            map.shift_remove(path);
        } else {
            map.insert(path.to_owned(), Value::Object(project));
        }
        any = true;
    }
    if any { projects } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn project(path: &str, key: &str) -> MergeKey {
        MergeKey::Project {
            path: path.into(),
            key: key.into(),
        }
    }

    fn server(name: &str) -> MergeKey {
        MergeKey::McpServer { name: name.into() }
    }

    /// The seeded `projects`: what the baseline holds, and the default too until it moves.
    fn projects() -> Value {
        json!({
            "/work/app": {"allowedTools": [], "hasTrustDialogAccepted": false},
            "/work/lib": {"allowedTools": ["Bash"]}
        })
    }

    fn servers() -> Value {
        json!({"local": {"command": "srv"}, "remote": {"url": "https://mcp.example"}})
    }

    fn untouched() -> MergeResult {
        MergeResult {
            projects: None,
            mcp_servers: None,
            applied: vec![],
            conflicts: vec![],
        }
    }

    /// `projects` alone, with no `mcpServers` on any side.
    fn on_projects(base: &Value, mine: &Value, theirs: &Value) -> MergeResult {
        three_way(
            (base, &Value::Null),
            (mine, &Value::Null),
            (theirs, &Value::Null),
        )
    }

    /// `mcpServers` alone.
    fn on_servers(base: &Value, mine: &Value, theirs: &Value) -> MergeResult {
        three_way(
            (&Value::Null, base),
            (&Value::Null, mine),
            (&Value::Null, theirs),
        )
    }

    fn names(v: &Value) -> Vec<&str> {
        v.as_object().unwrap().keys().map(String::as_str).collect()
    }

    #[test]
    fn a_profile_that_changed_nothing_changes_nothing_whatever_the_default_did() {
        let mut theirs = projects();
        theirs["/work/app"]["allowedTools"] = json!(["Read"]);
        theirs["/work/new"] = json!({"allowedTools": []});
        assert_eq!(
            three_way(
                (&projects(), &servers()),
                (&projects(), &servers()),
                (&theirs, &json!({}))
            ),
            untouched()
        );
    }

    #[test]
    fn a_key_changed_only_in_the_profile_is_applied_in_place() {
        let mut mine = projects();
        mine["/work/lib"]["allowedTools"] = json!(["Bash", "Edit"]);
        let mut theirs = projects();
        theirs["/work/app"]["hasTrustDialogAccepted"] = json!(true);
        let r = on_projects(&projects(), &mine, &theirs);
        let p = r.projects.unwrap();
        assert_eq!(p["/work/lib"]["allowedTools"], json!(["Bash", "Edit"]));
        assert_eq!(
            p["/work/app"]["hasTrustDialogAccepted"],
            json!(true),
            "the default's own change stays"
        );
        assert_eq!(
            names(&p),
            ["/work/app", "/work/lib"],
            "in the default's order"
        );
        assert_eq!(r.applied, [project("/work/lib", "allowedTools")]);
        assert!(r.conflicts.is_empty());
        assert_eq!(r.mcp_servers, None, "nothing changed under mcpServers");
    }

    #[test]
    fn a_key_removed_in_the_profile_goes_and_one_added_is_appended() {
        let mut mine = projects();
        mine["/work/app"]
            .as_object_mut()
            .unwrap()
            .shift_remove("hasTrustDialogAccepted");
        mine["/work/app"]["mcpServers"] = json!({"p": {"command": "x"}});
        let r = on_projects(&projects(), &mine, &projects());
        let p = r.projects.unwrap();
        assert_eq!(names(&p["/work/app"]), ["allowedTools", "mcpServers"]);
        assert_eq!(
            r.applied,
            [
                project("/work/app", "hasTrustDialogAccepted"),
                project("/work/app", "mcpServers")
            ]
        );
    }

    #[test]
    fn a_key_both_sides_changed_keeps_the_default_s_value() {
        let mut mine = projects();
        // Changed on both sides.
        mine["/work/app"]["allowedTools"] = json!(["Edit"]);
        // Changed in the profile, removed in the default.
        mine["/work/app"]["hasTrustDialogAccepted"] = json!(true);
        // Removed in the profile, changed in the default.
        mine["/work/lib"]
            .as_object_mut()
            .unwrap()
            .shift_remove("allowedTools");
        let mut theirs = projects();
        theirs["/work/app"]["allowedTools"] = json!(["Read"]);
        theirs["/work/app"]
            .as_object_mut()
            .unwrap()
            .shift_remove("hasTrustDialogAccepted");
        theirs["/work/lib"]["allowedTools"] = json!(["Bash", "Read"]);
        let r = on_projects(&projects(), &mine, &theirs);
        assert_eq!(r.projects, None, "nothing was applied");
        assert!(r.applied.is_empty());
        assert_eq!(
            r.conflicts,
            [
                project("/work/app", "allowedTools"),
                project("/work/app", "hasTrustDialogAccepted"),
                project("/work/lib", "allowedTools")
            ]
        );
    }

    #[test]
    fn the_same_change_on_both_sides_is_neither_applied_nor_a_conflict() {
        let mut mine = projects();
        mine["/work/app"]["hasTrustDialogAccepted"] = json!(true);
        mine["/work/new"] = json!({"allowedTools": []});
        let theirs = mine.clone();
        assert_eq!(on_projects(&projects(), &mine, &theirs), untouched());
    }

    #[test]
    fn a_whole_project_added_in_the_profile_is_appended_with_its_keys() {
        let mut mine = projects();
        mine["/work/new"] = json!({"allowedTools": ["Bash"], "hasTrustDialogAccepted": true});
        let mut theirs = projects();
        theirs["/work/other"] = json!({"allowedTools": []});
        let r = on_projects(&projects(), &mine, &theirs);
        let p = r.projects.unwrap();
        assert_eq!(
            names(&p),
            ["/work/app", "/work/lib", "/work/other", "/work/new"]
        );
        assert_eq!(p["/work/new"], mine["/work/new"]);
        assert_eq!(
            names(&p["/work/new"]),
            ["allowedTools", "hasTrustDialogAccepted"]
        );
        assert_eq!(
            r.applied,
            [
                project("/work/new", "allowedTools"),
                project("/work/new", "hasTrustDialogAccepted")
            ]
        );
    }

    #[test]
    fn a_whole_project_removed_in_the_profile_goes_unless_the_default_added_to_it() {
        let mut mine = projects();
        mine.as_object_mut().unwrap().shift_remove("/work/app");
        mine.as_object_mut().unwrap().shift_remove("/work/lib");
        let mut theirs = projects();
        theirs["/work/lib"]["hasTrustDialogAccepted"] = json!(true);
        let r = on_projects(&projects(), &mine, &theirs);
        assert_eq!(
            r.projects.unwrap(),
            json!({"/work/lib": {"hasTrustDialogAccepted": true}}),
            "the default's own key keeps its project"
        );
        assert_eq!(
            r.applied,
            [
                project("/work/app", "allowedTools"),
                project("/work/app", "hasTrustDialogAccepted"),
                project("/work/lib", "allowedTools")
            ]
        );
        assert!(r.conflicts.is_empty());
    }

    #[test]
    fn an_mcp_server_added_removed_or_changed_in_the_profile_is_applied() {
        let mine = json!({"local": {"command": "srv2"}, "added": {"command": "new"}});
        let r = on_servers(&servers(), &mine, &servers());
        let s = r.mcp_servers.unwrap();
        assert_eq!(s, mine);
        assert_eq!(names(&s), ["local", "added"]);
        assert_eq!(
            r.applied,
            [server("added"), server("local"), server("remote")]
        );
        assert_eq!(r.projects, None);
    }

    #[test]
    fn an_mcp_server_both_sides_changed_keeps_the_default_s() {
        let mine = json!({"local": {"command": "mine"}, "remote": {"url": "https://mcp.example"}});
        let theirs =
            json!({"local": {"command": "theirs"}, "remote": {"url": "https://mcp.example"}});
        let r = on_servers(&servers(), &mine, &theirs);
        assert_eq!(r.mcp_servers, None);
        assert!(r.applied.is_empty());
        assert_eq!(r.conflicts, [server("local")]);
    }

    #[test]
    fn a_default_without_the_subtrees_gets_the_profile_s_changes_alone() {
        let mine_p = json!({"/work/new": {"allowedTools": []}});
        let mine_s = json!({"added": {"command": "new"}});
        let r = three_way(
            (&json!({}), &json!({})),
            (&mine_p, &mine_s),
            (&Value::Null, &Value::Null),
        );
        assert_eq!(r.projects, Some(mine_p));
        assert_eq!(r.mcp_servers, Some(mine_s));
    }

    #[test]
    fn a_profile_without_the_subtrees_changed_nothing() {
        // A reset profile file has neither: no reason to remove every project and server.
        for (mine_p, mine_s) in [(Value::Null, Value::Null), (json!("reset"), json!(7))] {
            let r = three_way(
                (&projects(), &servers()),
                (&mine_p, &mine_s),
                (&projects(), &servers()),
            );
            assert_eq!(r, untouched(), "{mine_p} {mine_s}");
        }
    }

    #[test]
    fn a_default_value_that_is_not_an_object_takes_no_change() {
        let mut mine = projects();
        mine["/work/lib"]["allowedTools"] = json!(["Edit"]);
        mine["/work/new"] = json!({"allowedTools": []});
        let r = on_projects(&projects(), &mine, &json!(["not", "an", "object"]));
        assert_eq!(r.projects, None);
        assert!(r.applied.is_empty());
        assert_eq!(
            r.conflicts,
            [
                project("/work/lib", "allowedTools"),
                project("/work/new", "allowedTools")
            ]
        );
        let mut theirs = projects();
        theirs["/work/lib"] = json!("garbage");
        let r = on_projects(&projects(), &mine, &theirs);
        assert_eq!(r.conflicts, [project("/work/lib", "allowedTools")]);
        assert_eq!(r.applied, [project("/work/new", "allowedTools")]);
        assert_eq!(
            r.projects.unwrap()["/work/lib"],
            json!("garbage"),
            "the default's value is kept"
        );
    }

    #[test]
    fn an_explicitly_null_default_project_takes_no_change() {
        // A project the default set to null is present, not absent: it is a non-object value
        // like any other, so the profile's changes to it are conflicts and the null is kept.
        // A project the default lacks altogether still starts empty.
        let mut mine = projects();
        mine["/work/lib"]["hasTrustDialogAccepted"] = json!(true);
        let mut theirs = projects();
        theirs["/work/lib"] = Value::Null;
        let r = on_projects(&projects(), &mine, &theirs);
        assert_eq!(r.projects, None);
        assert!(r.applied.is_empty());
        assert_eq!(
            r.conflicts,
            [project("/work/lib", "hasTrustDialogAccepted")]
        );

        let mut theirs = projects();
        theirs.as_object_mut().unwrap().shift_remove("/work/lib");
        let r = on_projects(&projects(), &mine, &theirs);
        assert_eq!(r.applied, [project("/work/lib", "hasTrustDialogAccepted")]);
        assert_eq!(
            r.projects.unwrap()["/work/lib"],
            json!({"hasTrustDialogAccepted": true})
        );
    }
}
