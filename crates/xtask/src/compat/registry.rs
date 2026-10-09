//! Which checks a run makes. Each check belongs to one phase of the run, and the phases keep
//! the test account's single-use refresh lineage (R9) in one place at a time:
//! - `Standalone`: needs no account;
//! - `Profile`: the scratch default home holds no login, and tagteam runs the account in its
//!   session profile (`tagteam run`);
//! - `Live`: tagteam has made the account the scratch default home's live login.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    Standalone,
    Profile,
    Live,
}

/// A check that needs the user's own session, so it runs only when asked for (§15.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptIn {
    Ssh,
    LockedKeychain,
}

impl OptIn {
    pub fn flag(self) -> &'static str {
        match self {
            OptIn::Ssh => "--ssh HOST",
            OptIn::LockedKeychain => "--locked-keychain",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meta {
    /// Stable, for `--only` and the report.
    pub id: &'static str,
    /// The §15.4 bullet it settles, shortened.
    pub title: &'static str,
    pub phase: Phase,
    /// It needs the Keychain; on Linux it is reported as skipped.
    pub macos_only: bool,
    pub opt_in: Option<OptIn>,
}

/// The opt-in flags given.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags {
    pub ssh: bool,
    pub locked_keychain: bool,
}

impl Flags {
    fn has(self, o: OptIn) -> bool {
        match o {
            OptIn::Ssh => self.ssh,
            OptIn::LockedKeychain => self.locked_keychain,
        }
    }
}

/// The indices of the checks to run, in registry order. With `only`, exactly those, each named
/// once however often it is given; an unknown name, or an opt-in check without its flag, is a
/// usage error. Without it, every check that is not opt-in, and the opt-in ones whose flag is
/// given.
pub fn select(checks: &[Meta], only: &[String], flags: Flags) -> Result<Vec<usize>, String> {
    if only.is_empty() {
        return Ok((0..checks.len())
            .filter(|&i| checks[i].opt_in.is_none_or(|o| flags.has(o)))
            .collect());
    }
    let mut picked = Vec::new();
    for name in only {
        let Some(i) = checks.iter().position(|c| c.id == name) else {
            let known: Vec<&str> = checks.iter().map(|c| c.id).collect();
            return Err(format!(
                "no check named {name:?}; the checks are: {}",
                known.join(", ")
            ));
        };
        if let Some(o) = checks[i].opt_in.filter(|o| !flags.has(*o)) {
            return Err(format!("{name} runs only with {}", o.flag()));
        }
        picked.push(i);
    }
    picked.sort_unstable();
    picked.dedup();
    Ok(picked)
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn meta(id: &'static str, phase: Phase, opt_in: Option<OptIn>) -> Meta {
        Meta {
            id,
            title: "t",
            phase,
            macos_only: false,
            opt_in,
        }
    }

    const CHECKS: [Meta; 4] = [
        meta("probe", Phase::Standalone, None),
        meta("auth", Phase::Profile, None),
        meta("ssh", Phase::Live, Some(OptIn::Ssh)),
        meta("locked", Phase::Live, Some(OptIn::LockedKeychain)),
    ];

    fn only(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn by_default_every_check_runs_but_the_opt_in_ones() {
        assert_eq!(select(&CHECKS, &[], Flags::default()).unwrap(), [0, 1]);
        let ssh = Flags {
            ssh: true,
            ..Flags::default()
        };
        assert_eq!(select(&CHECKS, &[], ssh).unwrap(), [0, 1, 2]);
    }

    #[test]
    fn only_names_checks_in_registry_order_once_each() {
        assert_eq!(
            select(&CHECKS, &only(&["auth", "probe", "auth"]), Flags::default()).unwrap(),
            [0, 1]
        );
    }

    #[test]
    fn only_refuses_an_unknown_name_and_an_opt_in_check_without_its_flag() {
        let e = select(&CHECKS, &only(&["nope"]), Flags::default()).unwrap_err();
        assert!(e.contains("probe, auth, ssh, locked"), "{e}");
        let e = select(&CHECKS, &only(&["locked"]), Flags::default()).unwrap_err();
        assert_eq!(e, "locked runs only with --locked-keychain");
        let flags = Flags {
            locked_keychain: true,
            ..Flags::default()
        };
        assert_eq!(select(&CHECKS, &only(&["locked"]), flags).unwrap(), [3]);
    }
}
