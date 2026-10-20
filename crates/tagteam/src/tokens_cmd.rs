//! `tagteam tokens` (token report spec §11): the report of spec §10, after a refresh of the
//! index unless `--no-refresh`, as a table, JSON or CSV. It never fetches and touches no
//! Keychain item.

use std::collections::BTreeSet;
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use jiff::tz::TimeZone;
use serde_json::{Map, Value, json};
use tagteam_core::time::{format_iso8601, parse_iso8601_ms};
use tagteam_core::tokens::day_bounds_ms;
use tagteam_core::{ProviderId, printable};
use tagteam_engine::tokens::{
    ActiveBlock, Basis, Cost, IndexProgress, IndexState, PeriodKey, RowAccount, TokenCounts,
    TokenGrouping, TokenPeriod, TokenReport, TokenRow, TokenTotal, Wait, default_since_ms,
};
use tagteam_engine::words::{self, dollars, plural, scaled};
use tagteam_engine::{Engine, EngineError};

use crate::SCHEMA_VERSION;
use crate::cli::TokensArgs;
use crate::history;
use crate::render::{self, MISSING};

/// Spec §11.2: a refresh shows progress only once it has run this long.
const PROGRESS_AFTER: Duration = Duration::from_secs(1);

const BAD_SINCE: &str =
    "--since takes a span like 30d, 12h or 90m, a date YYYY-MM-DD, or an ISO 8601 time";
const BAD_UNTIL: &str =
    "--until takes a span like 30d, 12h or 90m, a date YYYY-MM-DD, or an ISO 8601 time";
const BAD_BY: &str = "--by takes account, model, account,model or none";
const PARTIAL_INDEX: &str = "the token index has not completed its first refresh, so this report is partial; run `tagteam tokens` without --no-refresh to complete it";

/// What `tokens`' flags ask for (spec §11.1), each checked before the index is touched.
pub(crate) struct Flags {
    pub(crate) grouping: TokenGrouping,
    pub(crate) tz: TimeZone,
    pub(crate) since_ms: i64,
    pub(crate) until_ms: Option<i64>,
}

/// Spec §11.1, §14: `--by`, `--tz`, `--since` and `--until`, or the usage error of the first
/// that is invalid. `--since` defaults to 30 days back, and for `month` to the first day of
/// the eleventh month before this one, in `--tz`. `--tz` defaults to the system zone, and to
/// UTC where that cannot be found; a zone named that the system's database lacks is an error.
pub(crate) fn parse_flags(
    args: &TokensArgs,
    period: &TokenPeriod,
    now_ms: i64,
) -> Result<Flags, String> {
    let grouping = match args.by.as_deref() {
        None => TokenGrouping::Account,
        Some(by) => grouping(by).ok_or(BAD_BY)?,
    };
    let tz = match args.tz.as_deref() {
        None => TimeZone::try_system().unwrap_or(TimeZone::UTC),
        Some(name) => zone(name).ok_or_else(|| format!("unknown time zone {name:?}"))?,
    };
    let since_ms = match args.since.as_deref() {
        Some(s) => parse_time(s, now_ms, &tz).ok_or(BAD_SINCE)?,
        None => default_since_ms(period, now_ms, &tz),
    };
    let until_ms = match args.until.as_deref() {
        Some(s) => Some(parse_time(s, now_ms, &tz).ok_or(BAD_UNTIL)?),
        None => None,
    };
    Ok(Flags {
        grouping,
        tz,
        since_ms,
        until_ms,
    })
}

/// `--by`'s four values (spec §11.1).
fn grouping(by: &str) -> Option<TokenGrouping> {
    match by {
        "account" => Some(TokenGrouping::Account),
        "model" => Some(TokenGrouping::Model),
        "account,model" => Some(TokenGrouping::AccountModel),
        "none" => Some(TokenGrouping::None),
        _ => None,
    }
}

/// `--tz`: `UTC`, or a zone the system's time zone database names (spec §11.1).
fn zone(name: &str) -> Option<TimeZone> {
    if name == "UTC" {
        return Some(TimeZone::UTC);
    }
    TimeZone::get(name).ok()
}

/// A `--since` or `--until` (spec §11.1), in epoch ms: `Nd`, `Nh` or `Nm` back from `now_ms`
/// (as `history` takes them), a date `YYYY-MM-DD` at its midnight in `tz`, or an ISO 8601 time.
pub(crate) fn parse_time(s: &str, now_ms: i64, tz: &TimeZone) -> Option<i64> {
    if let Some(span_s) = history::parse_since(s) {
        return now_ms.checked_sub(span_s.checked_mul(1000)?);
    }
    if let Some(date) = date(s) {
        return Some(day_bounds_ms(date, tz).0);
    }
    parse_iso8601_ms(s)
}

/// `YYYY-MM-DD` exactly, naming a date that exists.
fn date(s: &str) -> Option<jiff::civil::Date> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let part = |range: std::ops::Range<usize>| -> Option<i16> {
        let digits = s.get(range)?;
        if !digits.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok()
    };
    let (year, month, day) = (part(0..4)?, part(5..7)?, part(8..10)?);
    jiff::civil::Date::new(year, i8::try_from(month).ok()?, i8::try_from(day).ok()?).ok()
}

/// `--model` (spec §11.1): each pattern names every indexed model that contains it, ignoring
/// case, and the report takes those exact names. A pattern that names none is a usage error
/// that lists the indexed models.
pub(crate) fn resolve_models(
    patterns: &[String],
    indexed: &[String],
) -> Result<Vec<String>, String> {
    let mut names = BTreeSet::new();
    for pattern in patterns {
        let needle = pattern.to_lowercase();
        let mut found = false;
        for model in indexed
            .iter()
            .filter(|m| m.to_lowercase().contains(&needle))
        {
            names.insert(model.clone());
            found = true;
        }
        if !found {
            let have = if indexed.is_empty() {
                "none".to_owned()
            } else {
                indexed.join(", ")
            };
            return Err(format!(
                "--model {pattern} matches no indexed model; the index has {have}"
            ));
        }
    }
    Ok(names.into_iter().collect())
}

/// Core §13.1 for `tokens`: `--provider` when given, else the provider of the accounts named
/// (`named`: each one's position and provider), else `default`. Each ACCOUNT is resolved within
/// `--provider` first (core §10.4), so under it every one belongs to it; without it, accounts of
/// more than one provider are a usage error: a report covers one provider.
pub(crate) fn provider_for(
    flag: Option<&ProviderId>,
    default: &ProviderId,
    named: &[(u32, ProviderId)],
) -> Result<ProviderId, String> {
    if let Some(asked) = flag {
        return Ok(asked.clone());
    }
    let mut providers: Vec<&ProviderId> = Vec::new();
    for (_, p) in named {
        if !providers.contains(&p) {
            providers.push(p);
        }
    }
    match providers.as_slice() {
        [] => Ok(default.clone()),
        [one] => Ok((*one).clone()),
        several => Err(format!(
            "the accounts named belong to more than one provider ({}); name one provider's accounts",
            several
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Spec §5.3, §11.2: brings the index up to date before the report, waiting for a refresh
/// another process runs. Progress goes to stderr only when stderr is a terminal and the
/// refresh runs past a second. An interruption keeps what was committed (spec §14).
pub(crate) fn refresh(
    engine: &Engine,
    provider: &ProviderId,
    err: &mut dyn Write,
) -> Result<(), EngineError> {
    let mut progress = Progress::new(err, std::io::stderr().is_terminal(), Instant::now());
    let outcome = engine.refresh_tokens(provider, Wait::Wait, None, engine.cancel(), &mut |p| {
        progress.show(p, Instant::now())
    });
    progress.clear();
    outcome.map(|_| ())
}

/// Spec §11.2: whether a refresh that began at `started` shows its progress at `now`: only on a
/// terminal, and only once it has run `PROGRESS_AFTER`.
fn shows_progress(started: Instant, now: Instant, terminal: bool) -> bool {
    terminal && now.saturating_duration_since(started) >= PROGRESS_AFTER
}

/// Refresh progress (spec §11.2): one line on stderr, rewritten in place, when
/// `shows_progress` says so.
struct Progress<'w> {
    err: &'w mut dyn Write,
    terminal: bool,
    started: Instant,
    shown: bool,
}

impl<'w> Progress<'w> {
    fn new(err: &'w mut dyn Write, terminal: bool, started: Instant) -> Self {
        Progress {
            err,
            terminal,
            started,
            shown: false,
        }
    }

    /// Shows `p`, as of `now`.
    fn show(&mut self, p: &IndexProgress, now: Instant) {
        if !shows_progress(self.started, now, self.terminal) {
            return;
        }
        let _ = write!(
            self.err,
            "\r\x1b[2Krefreshing the token index: {}/{} files",
            p.files_done, p.files_total
        );
        let _ = self.err.flush();
        self.shown = true;
    }

    /// Erases the progress line, when one was shown.
    fn clear(&mut self) {
        if self.shown {
            let _ = write!(self.err, "\r\x1b[2K");
            let _ = self.err.flush();
        }
    }
}

/// Spec §11.3, §14: what goes to stderr beside the report. A `--no-refresh` report of an
/// index that never completed a refresh is partial, and says so; skipped lines and unreadable
/// files are the first sign of transcript format drift.
pub(crate) fn notices(report: &TokenReport, no_refresh: bool, app: &str) -> Vec<String> {
    let index = &report.index;
    let mut out = Vec::new();
    if no_refresh && index.refreshed_at_ms.is_none() {
        out.push(format!("note: {PARTIAL_INDEX}"));
    }
    if index.skipped_lines > 0 {
        out.push(format!(
            "warning: {} could not be read in the last refresh of the token index; {app}'s transcript format may have changed",
            plural(index.skipped_lines, "transcript line")
        ));
    }
    if index.unreadable_files > 0 {
        out.push(format!(
            "warning: {}",
            words::unreadable_transcripts(index.unreadable_files)
        ));
    }
    out
}

/// A time as JSON and CSV spell it: ISO 8601 UTC, to the second.
fn iso(ms: i64) -> String {
    format_iso8601(ms.div_euclid(1000))
}

/// Nano-dollars as US dollars, for JSON and CSV.
fn usd(nanos: u64) -> f64 {
    nanos as f64 / 1e9
}

fn period_name(p: &TokenPeriod) -> &'static str {
    match p {
        TokenPeriod::Day => "day",
        TokenPeriod::Month => "month",
        TokenPeriod::Session => "session",
        TokenPeriod::Block => "block",
    }
}

fn grouping_name(g: &TokenGrouping) -> &'static str {
    match g {
        TokenGrouping::None => "none",
        TokenGrouping::Account => "account",
        TokenGrouping::Model => "model",
        TokenGrouping::AccountModel => "account,model",
    }
}

fn basis_name(b: Basis) -> &'static str {
    match b {
        Basis::Profile => "profile",
        Basis::Timeline => "timeline",
        Basis::Unattributed => "unattributed",
    }
}

fn day_text(date: &jiff::civil::Date) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

fn month_text(year: i16, month: i8) -> String {
    format!("{year:04}-{month:02}")
}

/// Spec §11.2's `--json`: the report of spec §10 in camelCase (core §13.2), times in ISO 8601
/// UTC, dates and months in `timeZone`.
pub(crate) fn json(report: &TokenReport) -> Value {
    let q = &report.query;
    let rows: Vec<Value> = report.rows.iter().map(row_json).collect();
    json!({
        "schemaVersion": SCHEMA_VERSION,
        "provider": q.provider.as_str(),
        "period": period_name(&q.period),
        "grouping": grouping_name(&q.applied_grouping()),
        "timeZone": report.time_zone,
        "since": q.since_ms.map(iso),
        "until": q.until_ms.map(iso),
        "rows": rows,
        "total": total_json(&report.total),
        "index": index_json(&report.index),
        "unpricedModels": report.unpriced_models,
        "prices": {"asOf": report.prices_as_of, "overridden": report.price_overrides},
    })
}

fn row_json(row: &TokenRow) -> Value {
    let mut o = json!({"key": row.key.as_str(), "period": period_json(&row.period)});
    if let Some(a) = &row.account {
        o["account"] = account_json(a);
    }
    if let Some(m) = &row.model {
        o["model"] = json!(m);
    }
    let accounts: Vec<Value> = row.accounts.iter().map(account_json).collect();
    o["accounts"] = json!(accounts);
    o["models"] = json!(row.models);
    o["tokens"] = tokens_json(&row.counts);
    o["responses"] = json!(row.responses);
    o["unfinalized"] = json!(row.unfinalized);
    o["costUsd"] = cost_json(row.cost.as_ref());
    o["costPartial"] = json!(row.cost.as_ref().is_some_and(|c| c.partial));
    o
}

fn period_json(p: &PeriodKey) -> Value {
    match p {
        PeriodKey::Day { date } => json!({"kind": "day", "date": day_text(date)}),
        PeriodKey::Month { year, month } => {
            json!({"kind": "month", "month": month_text(*year, *month)})
        }
        PeriodKey::Session {
            session_id,
            project,
            first_ms,
            last_ms,
        } => json!({
            "kind": "session",
            "sessionId": session_id,
            "project": project,
            "first": iso(*first_ms),
            "last": iso(*last_ms),
        }),
        PeriodKey::Block {
            start_ms,
            end_ms,
            observed,
            active,
        } => json!({
            "kind": "block",
            "start": iso(*start_ms),
            "end": iso(*end_ms),
            "observed": observed,
            "active": active.as_ref().map(active_json),
        }),
    }
}

fn active_json(a: &ActiveBlock) -> Value {
    json!({
        "elapsedSeconds": a.elapsed_s,
        "remainingSeconds": a.remaining_s,
        "tokensPerMinute": a.tokens_per_min,
        "costPerHourUsd": cost_json(a.cost_per_hour.as_ref()),
        "projectedTokens": a.projected_tokens,
        "projectedCostUsd": cost_json(a.projected_cost.as_ref()),
    })
}

/// Spec §11.2: `{kind: "account", number, id, email, alias?}`, `{kind: "removed", id}`, or
/// `{kind: "unattributed"}`.
fn account_json(a: &RowAccount) -> Value {
    match a {
        RowAccount::Account {
            id,
            position,
            label,
            alias,
        } => {
            let mut o =
                json!({"kind": "account", "number": position, "id": id.as_str(), "email": label});
            if let Some(alias) = alias {
                o["alias"] = json!(alias);
            }
            o
        }
        RowAccount::Removed { id } => json!({"kind": "removed", "id": id.as_str()}),
        RowAccount::Unattributed => json!({"kind": "unattributed"}),
    }
}

fn tokens_json(c: &TokenCounts) -> Value {
    json!({
        "input": c.input,
        "output": c.output,
        "cacheWrite5m": c.cache_write_5m,
        "cacheWrite1h": c.cache_write_1h,
        "cacheWrite": c.cache_write(),
        "cacheRead": c.cache_read,
        "total": c.total(),
        "webSearch": c.web_search,
    })
}

fn cost_json(cost: Option<&Cost>) -> Value {
    cost.map_or(Value::Null, |c| json!(usd(c.usd_nanos)))
}

fn total_json(t: &TokenTotal) -> Value {
    let by_basis: Map<String, Value> = t
        .by_basis
        .iter()
        .map(|(basis, tokens)| (basis_name(*basis).to_owned(), json!(tokens)))
        .collect();
    json!({
        "tokens": tokens_json(&t.counts),
        "responses": t.responses,
        "unfinalized": t.unfinalized,
        "costUsd": cost_json(t.cost.as_ref()),
        "costPartial": t.cost.as_ref().is_some_and(|c| c.partial),
        "byBasis": by_basis,
    })
}

/// `IndexState` as spec §12's `token-index` event spells it, less the provider.
fn index_json(i: &IndexState) -> Value {
    json!({
        "refreshedAt": i.refreshed_at_ms.map(iso),
        "changedAt": i.changed_at_ms.map(iso),
        "files": i.files,
        "responses": i.responses,
        "skippedLines": i.skipped_lines,
        "unreadableFiles": i.unreadable_files,
        "oldest": i.oldest_ms.map(iso),
        "newest": i.newest_ms.map(iso),
    })
}

/// Spec §11.2's `--csv`: one line per report row, the period's own columns last. The account
/// is its email, `unattributed` or `removed:<id>`, and a field the rows are not grouped by is
/// empty.
pub(crate) fn csv(report: &TokenReport) -> String {
    let mut s = String::from(
        "period,account,model,input,output,cache_write_5m,cache_write_1h,cache_read,web_search,total,responses,unfinalized,cost_usd",
    );
    s.push_str(match report.query.period {
        TokenPeriod::Session => ",project,first_at,last_at",
        TokenPeriod::Block => ",start,end,observed,active",
        TokenPeriod::Day | TokenPeriod::Month => "",
    });
    s.push('\n');
    for row in &report.rows {
        let c = &row.counts;
        let mut fields = vec![
            csv_period(&row.period),
            row.account.as_ref().map_or_else(String::new, csv_account),
            row.model.clone().unwrap_or_default(),
            c.input.to_string(),
            c.output.to_string(),
            c.cache_write_5m.to_string(),
            c.cache_write_1h.to_string(),
            c.cache_read.to_string(),
            c.web_search.to_string(),
            c.total().to_string(),
            row.responses.to_string(),
            row.unfinalized.to_string(),
            row.cost
                .as_ref()
                .map_or_else(String::new, |cost| usd(cost.usd_nanos).to_string()),
        ];
        match &row.period {
            PeriodKey::Session {
                project,
                first_ms,
                last_ms,
                ..
            } => fields.extend([
                project.clone().unwrap_or_default(),
                iso(*first_ms),
                iso(*last_ms),
            ]),
            PeriodKey::Block {
                start_ms,
                end_ms,
                observed,
                active,
            } => fields.extend([
                iso(*start_ms),
                iso(*end_ms),
                observed.to_string(),
                active.is_some().to_string(),
            ]),
            PeriodKey::Day { .. } | PeriodKey::Month { .. } => {}
        }
        let quoted: Vec<String> = fields.iter().map(|f| history::csv_field(f)).collect();
        s.push_str(&quoted.join(","));
        s.push('\n');
    }
    s
}

fn csv_period(p: &PeriodKey) -> String {
    match p {
        PeriodKey::Day { date } => day_text(date),
        PeriodKey::Month { year, month } => month_text(*year, *month),
        PeriodKey::Session { session_id, .. } => session_id.clone(),
        PeriodKey::Block { start_ms, .. } => iso(*start_ms),
    }
}

fn csv_account(a: &RowAccount) -> String {
    match a {
        RowAccount::Account { label, .. } => label.clone(),
        RowAccount::Removed { id } => format!("removed:{}", id.as_str()),
        RowAccount::Unattributed => "unattributed".to_owned(),
    }
}

/// One column of the human table: its heading, and whether its cells are numbers, which are
/// right-aligned.
struct Col {
    head: &'static str,
    right: bool,
}

const fn left(head: &'static str) -> Col {
    Col { head, right: false }
}

const fn right(head: &'static str) -> Col {
    Col { head, right: true }
}

/// Spec §11.2's table: one row per report row, then the total, then the footnotes. Token
/// counts are scaled (`12.4M`) and costs are dollars with cents, `*` marking a partial one.
/// Session rows add their first and last times and their project, block rows their end and a
/// note. Times are in the report's zone.
pub(crate) fn human(report: &TokenReport) -> String {
    if report.rows.is_empty() {
        return format!("{}\n", words::NO_TOKEN_USAGE);
    }
    let q = &report.query;
    let by_model = report.rows.iter().any(|r| r.model.is_some());
    let mark = report
        .rows
        .iter()
        .filter_map(|r| r.cost.as_ref())
        .chain(report.total.cost.as_ref())
        .any(|c| c.partial);
    let mut cols = match q.period {
        TokenPeriod::Day => vec![left("DAY")],
        TokenPeriod::Month => vec![left("MONTH")],
        TokenPeriod::Session => vec![left("SESSION"), left("FIRST"), left("LAST")],
        TokenPeriod::Block => vec![left("START"), left("END")],
    };
    let lead = cols.len();
    cols.push(left("ACCOUNT"));
    if by_model {
        cols.push(left("MODEL"));
    }
    cols.extend([
        right("INPUT"),
        right("OUTPUT"),
        right("CACHE WRITE"),
        right("CACHE READ"),
        right("TOTAL"),
        right("API COST"),
    ]);
    let trailing = match q.period {
        TokenPeriod::Session => Some(left("PROJECT")),
        TokenPeriod::Block => Some(left("")),
        TokenPeriod::Day | TokenPeriod::Month => None,
    };
    let has_trailing = trailing.is_some();
    cols.extend(trailing);
    let mut rows: Vec<Vec<String>> = Vec::new();
    for row in &report.rows {
        let mut cells = period_cells(&row.period, &q.tz);
        cells.push(account_cell(row));
        if by_model {
            cells.push(row.model.clone().unwrap_or_else(|| row.models.join(", ")));
        }
        cells.extend(count_cells(&row.counts));
        cells.push(cost_cell(row.cost.as_ref(), mark));
        match &row.period {
            PeriodKey::Session { project, .. } => {
                cells.push(
                    project
                        .as_deref()
                        .map_or_else(|| MISSING.to_owned(), |p| printable(p).into_owned()),
                );
            }
            PeriodKey::Block {
                observed, active, ..
            } => cells.push(block_note(*observed, active.as_ref())),
            PeriodKey::Day { .. } | PeriodKey::Month { .. } => {}
        }
        rows.push(cells);
    }
    let mut total = vec!["TOTAL".to_owned()];
    total.resize(lead + 1 + usize::from(by_model), String::new());
    total.extend(count_cells(&report.total.counts));
    total.push(cost_cell(report.total.cost.as_ref(), mark));
    if has_trailing {
        total.push(String::new());
    }
    rows.push(total);
    format!("{}{}", table(&cols, &rows), footnotes(report))
}

/// The cell of `row` in column `i`, or nothing.
fn cell(row: &[String], i: usize) -> &str {
    row.get(i).map_or("", String::as_str)
}

/// Each column padded to its widest cell, two spaces apart, numbers right-aligned, as `list`
/// lays out its table (`render::width` counts a wide character as two).
fn table(cols: &[Col], rows: &[Vec<String>]) -> String {
    let widths: Vec<usize> = cols
        .iter()
        .enumerate()
        .map(|(i, c)| {
            rows.iter()
                .map(|r| render::width(cell(r, i)))
                .fold(render::width(c.head), usize::max)
        })
        .collect();
    let line = |texts: Vec<&str>| {
        let mut s = String::new();
        for (i, ((text, col), w)) in texts.iter().zip(cols).zip(&widths).enumerate() {
            if i > 0 {
                s.push_str("  ");
            }
            let gap = " ".repeat(w.saturating_sub(render::width(text)));
            if col.right {
                s.push_str(&gap);
                s.push_str(text);
            } else {
                s.push_str(text);
                s.push_str(&gap);
            }
        }
        format!("{}\n", s.trim_end())
    };
    let mut out = line(cols.iter().map(|c| c.head).collect());
    for r in rows {
        out.push_str(&line((0..cols.len()).map(|i| cell(r, i)).collect()));
    }
    out
}

fn period_cells(p: &PeriodKey, tz: &TimeZone) -> Vec<String> {
    match p {
        PeriodKey::Day { date } => vec![day_text(date)],
        PeriodKey::Month { year, month } => vec![month_text(*year, *month)],
        PeriodKey::Session {
            session_id,
            first_ms,
            last_ms,
            ..
        } => vec![
            session_id.clone(),
            local(*first_ms, tz),
            local(*last_ms, tz),
        ],
        PeriodKey::Block {
            start_ms, end_ms, ..
        } => vec![local(*start_ms, tz), local(*end_ms, tz)],
    }
}

/// `ms` in `tz`, to the minute.
fn local(ms: i64, tz: &TimeZone) -> String {
    jiff::Timestamp::from_millisecond(ms).map_or_else(
        |_| MISSING.to_owned(),
        |t| {
            t.to_zoned(tz.clone())
                .strftime("%Y-%m-%d %H:%M")
                .to_string()
        },
    )
}

/// The row's account (its position and label), or, when the rows are not grouped by account,
/// every account it holds.
fn account_cell(row: &TokenRow) -> String {
    match &row.account {
        Some(a) => account_text(a),
        None => row
            .accounts
            .iter()
            .map(account_text)
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn account_text(a: &RowAccount) -> String {
    match a {
        RowAccount::Account {
            position,
            label,
            alias,
            ..
        } => match alias {
            Some(alias) => format!("{position} {alias} ({label})"),
            None => format!("{position} {label}"),
        },
        RowAccount::Removed { id } => format!("removed {}", id.as_str()),
        RowAccount::Unattributed => "unattributed".to_owned(),
    }
}

fn count_cells(c: &TokenCounts) -> [String; 5] {
    [
        scaled(c.input),
        scaled(c.output),
        scaled(c.cache_write()),
        scaled(c.cache_read),
        scaled(c.total()),
    ]
}

/// A cost cell: dollars and cents, `*` when partial, `—` when nothing in it has a price. With
/// `mark` (some cost in the table is partial), a whole cost ends in a space, so the cents
/// line up.
fn cost_cell(cost: Option<&Cost>, mark: bool) -> String {
    let (text, partial) = match cost {
        Some(c) => (dollars(c.usd_nanos), c.partial),
        None => (MISSING.to_owned(), false),
    };
    match (partial, mark) {
        (true, _) => format!("{text}*"),
        (false, true) => format!("{text} "),
        (false, false) => text,
    }
}

/// A block row's note (spec §11.2): `observed` for a block a reset time placed (spec §9), and
/// for the active block its time left, burn rate and projection.
pub(crate) fn block_note(observed: bool, active: Option<&ActiveBlock>) -> String {
    let mut parts = Vec::new();
    if observed {
        parts.push("observed".to_owned());
    }
    if let Some(a) = active {
        let cost =
            |c: Option<&Cost>| c.map_or_else(|| MISSING.to_owned(), |c| dollars(c.usd_nanos));
        parts.push(format!(
            "active: {} left, {} tokens/min, {}/h, projected {} tokens, {}",
            render::duration(a.remaining_s),
            scaled(a.tokens_per_min.max(0.0).round() as u64),
            cost(a.cost_per_hour.as_ref()),
            scaled(a.projected_tokens),
            cost(a.projected_cost.as_ref()),
        ));
    }
    parts.join("; ")
}

/// Spec §11.2's footnotes, after a blank line: unpriced models, responses whose output is a
/// lower bound (spec §2.3), skipped lines, and the date of the price table. Unreadable files
/// are warned of on stderr instead (spec §11.3).
fn footnotes(report: &TokenReport) -> String {
    format!(
        "\n{}\n",
        words::token_footnotes(report, "*", false).join("\n")
    )
}

#[cfg(test)]
mod tests {
    use tagteam_engine::tokens::RowKey;

    use super::*;

    #[test]
    fn a_partial_cost_is_starred_and_a_whole_one_keeps_the_cents_in_line() {
        let partial = Cost {
            usd_nanos: 97_000_000,
            partial: true,
        };
        let whole = Cost {
            usd_nanos: 97_000_000,
            partial: false,
        };
        assert_eq!(cost_cell(Some(&partial), true), "$0.10*");
        assert_eq!(cost_cell(Some(&whole), true), "$0.10 ");
        assert_eq!(cost_cell(Some(&whole), false), "$0.10");
        assert_eq!(cost_cell(None, false), "—");
    }

    #[test]
    fn times_take_spans_dates_at_local_midnight_and_iso_times() {
        let now = 1_790_000_000_000; // 2026-09-21T14:13:20Z
        let utc = TimeZone::UTC;
        let kolkata = TimeZone::get("Asia/Kolkata").unwrap();
        assert_eq!(parse_time("30d", now, &utc), Some(now - 30 * 86_400_000));
        assert_eq!(parse_time("90m", now, &utc), Some(now - 5_400_000));
        assert_eq!(parse_time("2026-09-21", now, &utc), Some(1_789_948_800_000));
        assert_eq!(
            parse_time("2026-09-21", now, &kolkata),
            Some(1_789_948_800_000 - 19_800_000),
            "midnight in the zone, 05:30 ahead of UTC"
        );
        assert_eq!(
            parse_time("2026-09-21T14:13:20Z", now, &kolkata),
            Some(now),
            "an ISO time carries its own offset"
        );
        for bad in [
            "",
            "7w",
            "0d",
            "2026-02-30",
            "2026-9-21",
            "21-09-2026",
            "2026-09-21 14:13",
            "yesterday",
        ] {
            assert_eq!(parse_time(bad, now, &utc), None, "{bad:?}");
        }
    }

    fn args(tz: Option<&str>, by: Option<&str>) -> TokensArgs {
        TokensArgs {
            accounts: vec![],
            unattributed: false,
            model: vec![],
            since: None,
            until: None,
            by: by.map(str::to_owned),
            tz: tz.map(str::to_owned),
            csv: false,
            no_refresh: false,
        }
    }

    #[test]
    fn flags_take_their_defaults_and_refuse_what_they_cannot_read() {
        let now = 1_790_000_000_000;
        let day = |tz, by| parse_flags(&args(tz, by), &TokenPeriod::Day, now);
        let f = day(Some("UTC"), None).unwrap();
        assert_eq!((f.since_ms, f.until_ms), (now - 30 * 86_400_000, None));
        assert!(matches!(f.grouping, TokenGrouping::Account));
        assert!(matches!(
            day(None, Some("account,model")).unwrap().grouping,
            TokenGrouping::AccountModel
        ));
        assert_eq!(day(None, Some("week")).err().as_deref(), Some(BAD_BY));
        assert_eq!(
            day(Some("Mars/Olympus_Mons"), None).err().as_deref(),
            Some("unknown time zone \"Mars/Olympus_Mons\"")
        );
    }

    #[test]
    fn a_model_pattern_names_every_model_containing_it() {
        let indexed: Vec<String> = ["claude-opus-5-5", "claude-sonnet-5-5", "claude-test-9"]
            .map(str::to_owned)
            .to_vec();
        let pick = |p: &[&str]| {
            resolve_models(
                &p.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
                &indexed,
            )
        };
        assert_eq!(pick(&["OPUS"]).unwrap(), ["claude-opus-5-5"]);
        assert_eq!(
            pick(&["5-5", "opus"]).unwrap(),
            ["claude-opus-5-5", "claude-sonnet-5-5"],
            "sorted, each once"
        );
        assert_eq!(
            pick(&["opus", "nope"]).unwrap_err(),
            "--model nope matches no indexed model; the index has claude-opus-5-5, claude-sonnet-5-5, claude-test-9"
        );
        assert_eq!(
            resolve_models(&["x".to_owned()], &[]).unwrap_err(),
            "--model x matches no indexed model; the index has none"
        );
    }

    #[test]
    fn progress_shows_only_on_a_terminal_once_a_second_has_passed() {
        // Fixed instants: nothing here sleeps or reads how long anything took.
        let start = Instant::now();
        let later = |ms: u64| start + Duration::from_millis(ms);
        assert!(!shows_progress(start, later(999), true));
        assert!(shows_progress(start, later(1_000), true));
        assert!(
            !shows_progress(start, later(5_000), false),
            "never off a terminal"
        );
        assert!(
            !shows_progress(later(10), start, true),
            "a clock read before the start shows nothing"
        );
        let p = IndexProgress {
            files_done: 3,
            files_total: 9,
            ..IndexProgress::default()
        };
        let mut out = Vec::new();
        let mut progress = Progress::new(&mut out, true, start);
        progress.show(&p, later(500));
        progress.show(&p, later(1_500));
        progress.clear();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\r\x1b[2Krefreshing the token index: 3/9 files\r\x1b[2K"
        );
    }

    #[test]
    fn named_accounts_choose_the_provider() {
        // Core §13.1: --provider, else the named accounts' provider, else the default.
        let (cc, fake) = (
            ProviderId::new("claude-code"),
            ProviderId::new("fake-agent"),
        );
        let named = |list: &[(u32, &ProviderId)]| {
            list.iter()
                .map(|(n, p)| (*n, (*p).clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(provider_for(None, &cc, &[]), Ok(cc.clone()));
        assert_eq!(provider_for(Some(&fake), &cc, &[]), Ok(fake.clone()));
        assert_eq!(
            provider_for(None, &cc, &named(&[(2, &fake)])),
            Ok(fake.clone())
        );
        assert_eq!(
            provider_for(Some(&cc), &cc, &named(&[(1, &cc), (2, &cc)])),
            Ok(cc.clone())
        );
        assert_eq!(
            provider_for(None, &cc, &named(&[(1, &cc), (2, &fake), (3, &cc)])),
            Err("the accounts named belong to more than one provider (claude-code, fake-agent); name one provider's accounts".to_owned())
        );
    }

    #[test]
    fn a_block_note_says_observed_and_what_the_active_block_projects() {
        assert_eq!(block_note(false, None), "");
        assert_eq!(block_note(true, None), "observed");
        let active = ActiveBlock {
            elapsed_s: 9_000,
            remaining_s: 9_000,
            tokens_per_min: 41_234.6,
            cost_per_hour: Some(Cost {
                usd_nanos: 3_200_000_000,
                partial: false,
            }),
            projected_tokens: 9_800_000,
            projected_cost: None,
        };
        assert_eq!(
            block_note(true, Some(&active)),
            "observed; active: 2h30m left, 41.2K tokens/min, $3.20/h, projected 9.8M tokens, —"
        );
    }

    fn report_with(index: IndexState) -> TokenReport {
        TokenReport {
            query: tagteam_engine::tokens::TokenQuery {
                provider: ProviderId::new("claude-code"),
                period: TokenPeriod::Day,
                grouping: TokenGrouping::Account,
                since_ms: None,
                until_ms: None,
                accounts: None,
                models: None,
                session: None,
                tz: TimeZone::UTC,
            },
            time_zone: "UTC".to_owned(),
            rows: vec![],
            total: TokenTotal::default(),
            index,
            unpriced_models: vec![],
            prices_as_of: "2026-10-01",
            price_overrides: vec![],
        }
    }

    #[test]
    fn notices_name_a_partial_index_skipped_lines_and_unreadable_files() {
        let refreshed = IndexState {
            refreshed_at_ms: Some(1),
            ..IndexState::default()
        };
        assert!(notices(&report_with(refreshed), true, "Claude Code").is_empty());
        assert_eq!(
            notices(&report_with(IndexState::default()), true, "Claude Code"),
            [format!("note: {PARTIAL_INDEX}")]
        );
        assert!(
            notices(&report_with(IndexState::default()), false, "Claude Code").is_empty(),
            "a refresh that ran says nothing of being partial"
        );
        let drift = IndexState {
            skipped_lines: 1,
            unreadable_files: 3,
            ..refreshed
        };
        assert_eq!(
            notices(&report_with(drift), false, "Claude Code"),
            [
                "warning: 1 transcript line could not be read in the last refresh of the token index; Claude Code's transcript format may have changed",
                "warning: 3 transcript files could not be read; the next refresh tries again",
            ]
        );
        assert!(
            notices(&report_with(drift), false, "FakeAgent")[0]
                .ends_with("FakeAgent's transcript format may have changed")
        );
        let many = IndexState {
            skipped_lines: 12,
            unreadable_files: 1,
            ..refreshed
        };
        let out = notices(&report_with(many), false, "Claude Code");
        assert!(out[0].starts_with("warning: 12 transcript lines could not be read"));
        assert!(out[1].starts_with("warning: 1 transcript file could not be read"));
    }

    #[test]
    fn a_session_rows_project_is_printable_in_the_table_and_raw_in_json_and_csv() {
        let raw = "/tmp/\u{1b}[2J\u{7}evil";
        let period = PeriodKey::Session {
            session_id: "s1".to_owned(),
            project: Some(raw.to_owned()),
            first_ms: 1_790_000_000_000,
            last_ms: 1_790_000_060_000,
        };
        let row = TokenRow {
            key: RowKey::new(&period, None, None),
            period,
            account: None,
            model: None,
            accounts: vec![],
            models: vec![],
            counts: TokenCounts::default(),
            responses: 1,
            unfinalized: 0,
            cost: None,
        };
        let mut report = report_with(IndexState::default());
        report.query.period = TokenPeriod::Session;
        report.rows = vec![row.clone()];

        let table = human(&report);
        assert!(table.contains("/tmp/\u{fffd}[2J\u{fffd}evil"), "{table:?}");
        assert!(!table.contains('\u{1b}'), "{table:?}");
        assert_eq!(row_json(&row)["period"]["project"], raw);
        assert!(csv(&report).contains(raw), "{:?}", csv(&report));
    }
}
