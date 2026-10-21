//! The STATUS column's phrase, and what a row without a reading says across its windows
//! (redesign §6.3, T3-11, T3-14).

use tagteam_engine::views::{AccountView, UsageStatus};
use tagteam_engine::words::{self, Problem};

use crate::theme::Role;

/// `✕`: the account needs you.
const NEEDS_YOU: &str = "✕";
/// `!`: tagteam retries.
const RETRIES: &str = "!";
/// `○`: the account is disabled.
pub(super) const DISABLED: &str = "○";
/// What an API key's row adds to its status words, in place of its windows.
const NO_LIMITS: &str = "has no usage limits to show";

/// A STATUS phrase: its glyph, when it has one, its words and its role.
struct Phrase {
    glyph: Option<&'static str>,
    words: &'static str,
    role: Role,
}

/// A problem's glyph and role: `✕` crit for a login, `!` warn for what tagteam retries or an old
/// reading; `NotChecked` has no glyph and is muted.
fn mark(problem: Problem) -> (Option<&'static str>, Role) {
    match problem {
        Problem::NeedsLogin | Problem::LoginExpired | Problem::NoLogin => {
            (Some(NEEDS_YOU), Role::Crit)
        }
        Problem::RateLimited
        | Problem::Keychain
        | Problem::Budget
        | Problem::OtherAccount
        | Problem::Retrying
        | Problem::OldReading => (Some(RETRIES), Role::Warn),
        Problem::NotChecked => (None, Role::Muted),
    }
}

/// A problem's STATUS words (§6.3); `NotChecked` has none, as the window columns say it.
fn problem_words(problem: Problem) -> Option<&'static str> {
    Some(match problem {
        Problem::NeedsLogin => "new login",
        Problem::LoginExpired => "expired",
        Problem::NoLogin => "no login",
        Problem::RateLimited => "rate-limited",
        Problem::Keychain => "keychain",
        Problem::Budget => "budget",
        Problem::OtherAccount | Problem::Retrying => "retrying",
        Problem::OldReading => "old reading",
        Problem::NotChecked => return None,
    })
}

/// The first row of §6.3's table that applies, with T3-14's `▶ this session`.
fn first(v: &AccountView, this: bool, best: bool) -> Option<Phrase> {
    if let Some(problem) = words::problem(v) {
        if let Some(words) = problem_words(problem) {
            let (glyph, role) = mark(problem);
            return Some(Phrase { glyph, words, role });
        }
    }
    let (glyph, words, role) = if v.row.disabled {
        (Some(DISABLED), "disabled", Role::Muted)
    } else if this {
        (Some(words::IN_SESSION), "this session", Role::Muted)
    } else if v.active {
        (None, "in use", Role::Accent)
    } else if best {
        (None, "best", Role::Text)
    } else if v.usage.status == UsageStatus::ApiKey {
        (None, "api key", Role::Muted)
    } else {
        return None;
    };
    Some(Phrase { glyph, words, role })
}

/// The STATUS column's phrase for a row, first match (§6.3, T3-14), with its role: `✕ new
/// login` crit … `api key` muted. `this` is the run shell's account; `best` the forecast's pick.
pub(crate) fn phrase(v: &AccountView, this: bool, best: bool) -> Option<(String, Role)> {
    let p = first(v, this, best)?;
    let text = match p.glyph {
        Some(glyph) => format!("{glyph} {}", p.words),
        None => p.words.to_owned(),
    };
    Some((text, p.role))
}

/// The phrase's glyph alone, for under 100 columns: `✕`, `!` or `○`.
pub(crate) fn glyph(v: &AccountView, this: bool, best: bool) -> Option<(&'static str, Role)> {
    let p = first(v, this, best)?;
    p.glyph
        .filter(|g| [NEEDS_YOU, RETRIES, DISABLED].contains(g))
        .map(|g| (g, p.role))
}

/// A row with no reading, across its window columns (§6.3): `✕ needs a new login`,
/// `api key · has no usage limits to show`, `! can't read the Keychain · retry in 2m`.
pub(crate) fn unread_words(v: &AccountView, now_s: i64) -> (String, Role) {
    let u = &v.usage;
    let mut parts = vec![words::status_words(u)];
    if u.status == UsageStatus::ApiKey {
        parts.push(NO_LIMITS.to_owned());
    }
    if let Some(at) = u.retry_at.filter(|&at| at > now_s) {
        parts.push(format!("retry in {}", words::duration(at - now_s)));
    }
    let text = parts.join(" · ");
    match words::problem(v).map(mark) {
        Some((Some(glyph), role)) => (format!("{glyph} {text}"), role),
        _ => (text, Role::Muted),
    }
}
