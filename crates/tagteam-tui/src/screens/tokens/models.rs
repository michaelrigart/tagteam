//! Short model IDs (T5-22): the Tokens table and the breadcrumbs name a model by what tells it
//! apart from the report's other models; the detail, the filter dialog, the bar and the
//! footnotes name it in full. The rule knows no provider's naming (ARCH-1): it works on
//! separators and date stamps only.

use std::collections::{HashMap, HashSet};

use tagteam_engine::tokens::TokenReport;

/// What ends a model ID's first segment.
const SEPARATORS: [char; 5] = ['-', '.', '/', ':', '_'];

/// T5-22: the short form of each of a report's models.
#[cfg_attr(test, derive(Debug, Clone, PartialEq))]
pub(super) struct ShortModels(HashMap<String, String>);

impl ShortModels {
    /// The short forms of `ids`: each without a trailing date stamp, then without the first
    /// segment every one of them shares and is longer than. A short form that is empty or reads
    /// like another model's shown ID gives way to the full ID, until none does.
    pub(super) fn of<'a>(ids: impl IntoIterator<Item = &'a str>) -> ShortModels {
        let ids: Vec<&str> = ids
            .into_iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let stems: Vec<&str> = ids.iter().map(|id| stem(id)).collect();
        let lead = shared_lead(&stems);
        let mut shown: Vec<&str> = stems.iter().map(|s| &s[lead..]).collect();
        loop {
            let alike: Vec<usize> = (0..ids.len())
                .filter(|&i| {
                    shown[i] != ids[i]
                        && (shown[i].is_empty()
                            || (0..ids.len()).any(|j| j != i && shown[j] == shown[i]))
                })
                .collect();
            if alike.is_empty() {
                break;
            }
            for i in alike {
                shown[i] = ids[i];
            }
        }
        ShortModels(
            ids.into_iter()
                .zip(shown)
                .map(|(id, short)| (id.to_owned(), short.to_owned()))
                .collect(),
        )
    }

    /// The short forms of `report`'s models: its rows' `model` and `models`.
    pub(super) fn of_report(report: &TokenReport) -> ShortModels {
        ShortModels::of(
            report
                .rows
                .iter()
                .flat_map(|r| r.model.iter().chain(&r.models))
                .map(String::as_str),
        )
    }

    /// `id`'s short form, or `id` itself.
    pub(super) fn get<'a>(&'a self, id: &'a str) -> &'a str {
        self.0.get(id).map_or(id, String::as_str)
    }
}

/// `id` without a trailing date stamp: `-` or `@`, then 8 ASCII digits or `YYYY-MM-DD`, after at
/// least one character.
fn stem(id: &str) -> &str {
    let bytes = id.as_bytes();
    let digits = |s: &[u8]| s.iter().all(u8::is_ascii_digit);
    let date = |s: &[u8]| match s.len() {
        8 => digits(s),
        10 => {
            digits(&s[..4]) && s[4] == b'-' && digits(&s[5..7]) && s[7] == b'-' && digits(&s[8..])
        }
        _ => false,
    };
    for n in [8, 10] {
        if bytes.len() > n + 1 {
            let at = bytes.len() - n - 1;
            if matches!(bytes[at], b'-' | b'@') && date(&bytes[at + 1..]) {
                return &id[..at];
            }
        }
    }
    id
}

/// How long the first segment of `stems` is, up to and including the first separator, when
/// every stem starts with that same segment and is longer than it; else 0.
fn shared_lead(stems: &[&str]) -> usize {
    let Some(lead) = stems
        .first()
        .and_then(|s| s.find(SEPARATORS).map(|at| &s[..=at]))
    else {
        return 0;
    };
    if stems
        .iter()
        .all(|s| s.starts_with(lead) && s.len() > lead.len())
    {
        lead.len()
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn short(ids: &[&str]) -> Vec<String> {
        let models = ShortModels::of(ids.iter().copied());
        ids.iter().map(|id| models.get(id).to_owned()).collect()
    }

    #[test]
    fn short_ids_drop_a_date_stamp_and_a_shared_lead_and_stay_apart() {
        assert_eq!(
            short(&[
                "claude-opus-4-1-20250805",
                "claude-sonnet-4-5-20250929",
                "claude-haiku-4-5-20251001",
            ]),
            ["opus-4-1", "sonnet-4-5", "haiku-4-5"]
        );
        assert_eq!(short(&["claude-opus-4-1-20250805"]), ["opus-4-1"]);
        assert_eq!(
            short(&["claude-opus-4-1-20250805", "claude-opus-4-1-20250901"]),
            ["claude-opus-4-1-20250805", "claude-opus-4-1-20250901"],
            "alike once short: both full"
        );
        assert_eq!(
            short(&["claude-opus-4-1-20250805", "claude-opus-4-1"]),
            ["claude-opus-4-1-20250805", "claude-opus-4-1"]
        );
        assert_eq!(
            short(&["claude-opus-5-5", "gpt-5"]),
            ["claude-opus-5-5", "gpt-5"],
            "no shared lead"
        );
        assert_eq!(
            short(&["gpt-4o-2024-08-06", "gpt-4o-mini"]),
            ["4o", "4o-mini"]
        );
        assert_eq!(short(&["vendor.model-x@20250805"]), ["model-x"]);
        assert_eq!(short(&["opus"]), ["opus"]);
        assert_eq!(short(&["claude-"]), ["claude-"]);
        assert_eq!(
            short(&["v-m-1", "v-m-1-20250805", "v-v-m-1", "v-n"]),
            ["v-m-1", "v-m-1-20250805", "v-v-m-1", "n"],
            "`v-v-m-1`'s `v-m-1` reads like `v-m-1` once that shows in full"
        );
        let models = ShortModels::of(["claude-opus-4-1-20250805"]);
        assert_eq!(models.get("gpt-5"), "gpt-5", "not in the report");
    }
}
