//! The renderer (spec §4.2, §5.1): `view` draws the state and reads nothing else.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tagteam_engine::words::Severity;
use unicode_width::UnicodeWidthStr;

use crate::actions;
use crate::screens;
use crate::state::{Screen, State};

/// Below this the body is replaced by `TOO_SMALL` (spec §5.1).
pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;
pub const TOO_SMALL: &str = "terminal too small (80×24 needed)";
/// The header's mode in timer mode (spec §5.1); part C adds live mode.
pub const MODE_TIMER: &str = "○ refresh 30s";
/// The spinner's frames, one per tick while an action runs.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The footer's keys for each screen, in the order they are dropped from the end when the
/// line is too narrow (spec §5.6). Help lists every key.
const ACCOUNTS_KEYS: [(&str, &str); 10] = [
    ("s", "switch"),
    ("b", "best"),
    ("n", "next"),
    ("e", "enable/disable"),
    ("a", "alias"),
    ("m", "move"),
    ("x", "remove"),
    ("r", "refresh"),
    ("R", "run"),
    ("Enter", "history"),
];
const HISTORY_KEYS: [(&str, &str); 4] = [
    ("j/k", "account"),
    ("[ ]", "range"),
    ("r", "refresh"),
    ("1", "accounts"),
];

/// Where each part of the screen goes.
pub(crate) struct Areas {
    pub(crate) header: Rect,
    pub(crate) banner: Option<Rect>,
    pub(crate) body: Rect,
    pub(crate) footer: Rect,
}

/// The header on the first row, the banner (when there is one) below it, the footer on the
/// last rows, and the body between. The banner, and a toast in the footer, take as many rows as
/// their words need at the width, `WRAP_ROWS` at most, from the body, which keeps a row at
/// least. Part A has no activity line: timer mode hides it (§5.1).
pub(crate) fn areas(state: &State, area: Rect) -> Areas {
    let width = usize::from(area.width);
    let spare = area.height.saturating_sub(3);
    let rows = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
    let banner = banner(state).map_or(0, |(text, _)| rows(banner_rows(&text, width).len()));
    let banner = banner.min(spare);
    let footer = toast_rows(state, width).map_or(1, |t| rows(t.len()));
    let footer = footer.min(spare - banner + 1).min(area.height);
    let top = area.y + 1.min(area.height) + banner;
    Areas {
        header: Rect::new(area.x, area.y, area.width, 1.min(area.height)),
        banner: (banner > 0).then(|| Rect::new(area.x, area.y + 1, area.width, banner)),
        body: Rect::new(
            area.x,
            top,
            area.width,
            area.height.saturating_sub(top - area.y + footer),
        ),
        footer: Rect::new(
            area.x,
            area.y + area.height.saturating_sub(footer),
            area.width,
            footer,
        ),
    }
}

/// The body's area at the terminal's last known size, for the keys that page or scroll.
pub(crate) fn body_at(state: &State) -> Rect {
    let (w, h) = state.size;
    areas(state, Rect::new(0, 0, w, h)).body
}

/// `head`, then `parts`, two spaces apart, in lines of at most `width` columns (R20). A part
/// that does not fit goes on to a continuation line that starts `indent` columns in. One wider
/// than a whole continuation line breaks at its own ` · `s, and only a piece wider even than
/// that is cut (`fit`). A line with nothing past `indent` takes its first part unseparated.
pub(crate) fn flow(
    head: Vec<Span<'static>>,
    parts: Vec<String>,
    indent: usize,
    width: usize,
) -> Vec<Line<'static>> {
    let used = head.iter().map(|s| width_of(&s.content)).sum();
    let mut flow = Flow {
        lines: Vec::new(),
        spans: head,
        used,
        indent,
        width,
    };
    for part in parts {
        if flow.fits("  ", &part) || width_of(&part) <= width.saturating_sub(indent) {
            flow.put("  ", part);
            continue;
        }
        for (i, piece) in part.split(" · ").enumerate() {
            flow.put(if i == 0 { "  " } else { " · " }, piece.to_owned());
        }
    }
    flow.lines.push(Line::from(flow.spans));
    flow.lines
}

/// `flow`'s lines so far, and the one it is filling.
struct Flow {
    lines: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    used: usize,
    indent: usize,
    width: usize,
}

impl Flow {
    /// Nothing on this line yet past the indent.
    fn fresh(&self) -> bool {
        self.used <= self.indent
    }

    fn fits(&self, sep: &str, text: &str) -> bool {
        let sep = if self.fresh() { 0 } else { width_of(sep) };
        self.used + sep + width_of(text) <= self.width
    }

    fn put(&mut self, sep: &str, text: String) {
        if !self.fits(sep, &text) && !self.fresh() {
            let next = vec![Span::raw(" ".repeat(self.indent))];
            self.lines
                .push(Line::from(std::mem::replace(&mut self.spans, next)));
            self.used = self.indent;
        }
        let text = if self.fresh() {
            fit(&text, self.width.saturating_sub(self.used))
        } else {
            format!("{sep}{text}")
        };
        self.used += width_of(&text);
        self.spans.push(Span::raw(text));
    }
}

/// The windows a list scrolled to window `from` shows in `rows` lines, given each window's
/// height (R20): whole windows only, and at least one. `from` goes no further than
/// `furthest_scroll`.
pub(crate) fn scrolled_windows(
    heights: &[usize],
    from: usize,
    rows: usize,
) -> std::ops::Range<usize> {
    let from = from.min(furthest_scroll(heights, rows));
    let (mut to, mut used) = (from, 0);
    while to < heights.len() && (to == from || used + heights[to] <= rows) {
        used += heights[to];
        to += 1;
    }
    from..to
}

/// How far `J` scrolls: the first window from which every later one fits in `rows` lines, so
/// the last window shows.
pub(crate) fn furthest_scroll(heights: &[usize], rows: usize) -> usize {
    let (mut from, mut used) = (heights.len(), 0);
    while from > 0 && used + heights[from - 1] <= rows {
        used += heights[from - 1];
        from -= 1;
    }
    from.min(heights.len().saturating_sub(1))
}

/// The banner's text, and whether it is an error: a failed read (spec §10.4) first, then on
/// Tokens a failed token read (B12), else what the accounts view has to say (a run shell, an
/// unmanaged live login).
pub(crate) fn banner(state: &State) -> Option<(String, bool)> {
    let tokens = (state.screen == Screen::Tokens)
        .then(|| state.tokens.error())
        .flatten();
    match (&state.banner, tokens) {
        (Some(error), _) => Some((error.clone(), true)),
        (None, Some(error)) => Some((error.to_owned(), true)),
        (None, None) => screens::accounts::banner(&state.accounts).map(|text| (text, false)),
    }
}

/// The banner and a toast take at most this many rows (spec §5.1).
const WRAP_ROWS: usize = 3;

/// `text` in rows of at most `width` columns, broken at word boundaries (`wrap`), at most `max`
/// of them. Only text beyond the last row is cut, which then ends in `…`.
pub(crate) fn wrap_rows(text: &str, width: usize, max: usize) -> Vec<String> {
    let mut rows = wrap(text, width);
    if rows.len() > max.max(1) {
        let last = max.max(1) - 1;
        let rest = rows[last..].join(" ");
        rows.truncate(last);
        rows.push(fit(&rest, width));
    }
    rows
}

/// The banner's rows: one column in, as the header's text is.
fn banner_rows(text: &str, width: usize) -> Vec<String> {
    wrap_rows(text, width.saturating_sub(1), WRAP_ROWS)
        .into_iter()
        .map(|row| format!(" {row}"))
        .collect()
}

/// The toast's rows, when the footer shows it: not once the shutdown has begun.
fn toast_rows(state: &State, width: usize) -> Option<Vec<String>> {
    let toast = state.toast.as_ref().filter(|_| state.quitting.is_none())?;
    let mut rows = wrap_rows(&toast.text, width, WRAP_ROWS);
    if rows.is_empty() {
        rows.push(String::new());
    }
    Some(rows)
}

pub fn view(state: &State, frame: &mut Frame, now_ms: i64) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(Paragraph::new(TOO_SMALL), area);
        return;
    }
    let areas = areas(state, area);
    frame.render_widget(Paragraph::new(header(state, area.width)), areas.header);
    if let (Some(rect), Some((text, error))) = (areas.banner, banner(state)) {
        // An error takes the critical severity's style, anything else the warning's.
        let severity = if error {
            Severity::Critical
        } else {
            Severity::Warning
        };
        let style = severity_style(state.color, severity);
        let rows: Vec<Line<'static>> = banner_rows(&text, usize::from(area.width))
            .into_iter()
            .map(|row| Line::from(Span::styled(row, style)))
            .collect();
        frame.render_widget(Paragraph::new(rows), rect);
    }
    match state.screen {
        Screen::Accounts => screens::accounts::render(state, frame, areas.body, now_ms),
        Screen::Tokens => screens::tokens::render(state, frame, areas.body, now_ms),
        Screen::History => screens::history::render(state, frame, areas.body, now_ms),
    }
    frame.render_widget(
        Paragraph::new(footer(state, area.width, now_ms)),
        areas.footer,
    );
    actions::render_overlay(state, frame, now_ms);
}

/// ` tagteam  [1 Accounts]  2 Tokens  3 History` on the left, the mode and `? help` on the
/// right.
fn header(state: &State, width: u16) -> Line<'static> {
    let mut spans = vec![Span::styled(" tagteam", bold())];
    for (screen, label) in [
        (Screen::Accounts, "1 Accounts"),
        (Screen::Tokens, "2 Tokens"),
        (Screen::History, "3 History"),
    ] {
        spans.push(Span::raw("  "));
        if screen == state.screen {
            spans.push(Span::styled(format!("[{label}]"), bold()));
        } else {
            spans.push(Span::raw(label));
        }
    }
    let right = format!("{MODE_TIMER}   ? help ");
    let used: usize = spans.iter().map(|s| width_of(&s.content)).sum();
    let gap = usize::from(width).saturating_sub(used + width_of(&right));
    spans.push(Span::raw(" ".repeat(gap.max(1))));
    spans.push(Span::raw(right));
    Line::from(spans)
}

/// The footer: `finishing <action>…` once the shutdown has begun, else the toast (on as many
/// rows as it needs, `WRAP_ROWS` at most), else the running action's spinner, else the screen's
/// keys (spec §5.1, §10.2 step 4).
fn footer(state: &State, width: u16, now_ms: i64) -> Vec<Line<'static>> {
    if state.quitting.is_some() {
        let text = match &state.running {
            Some(action) => format!("finishing {}…", action.name()),
            None => "finishing…".to_owned(),
        };
        return vec![Line::from(Span::styled(text, bold()))];
    }
    if let Some(rows) = toast_rows(state, usize::from(width)) {
        return rows
            .into_iter()
            .map(|row| Line::from(Span::styled(row, bold())))
            .collect();
    }
    if let Some(action) = &state.running {
        let frame = SPINNER[(now_ms.div_euclid(250)).rem_euclid(10) as usize];
        return vec![Line::from(format!("{frame} {}…", action.name()))];
    }
    let keys: Vec<(&str, &str)> = match state.screen {
        Screen::Accounts => ACCOUNTS_KEYS.to_vec(),
        Screen::Tokens => screens::tokens::footer_keys(&state.tokens),
        Screen::History => HISTORY_KEYS.to_vec(),
    };
    let mut text = String::new();
    for (key, what) in keys {
        let item = format!("{}{key} {what}", if text.is_empty() { "" } else { "  " });
        if width_of(&text) + width_of(&item) > usize::from(width) {
            break;
        }
        text.push_str(&item);
    }
    vec![Line::from(text)]
}

/// The columns `s` takes on a terminal: a CJK character or an emoji takes two.
pub(crate) fn width_of(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// `s` cut to at most `w` columns by display width, never by bytes, ending in `…` when cut.
pub(crate) fn fit(s: &str, w: usize) -> String {
    if width_of(s) <= w {
        return s.to_owned();
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    for c in s.chars() {
        out.push(c);
        if width_of(&out) > w - 1 {
            out.pop();
            break;
        }
    }
    out.push('…');
    out
}

/// `s` cut to `w` columns and padded to exactly `w`.
pub(crate) fn pad(s: &str, w: usize) -> String {
    let s = fit(s, w);
    let fill = w.saturating_sub(width_of(&s));
    format!("{s}{}", " ".repeat(fill))
}

/// `text` broken into lines of at most `w` columns, at spaces where it can be.
pub(crate) fn wrap(text: &str, w: usize) -> Vec<String> {
    let w = w.max(1);
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let first = lines.len();
        let mut line = String::new();
        for word in paragraph.split(' ') {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            if width_of(&candidate) <= w {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            // A word wider than a line is cut where the width runs out, at least one character
            // a line.
            let mut rest = word;
            while width_of(rest) > w {
                let mut cut = 0;
                for (i, c) in rest.char_indices() {
                    if width_of(&rest[..i + c.len_utf8()]) > w {
                        break;
                    }
                    cut = i + c.len_utf8();
                }
                if cut == 0 {
                    cut = rest.chars().next().map_or(rest.len(), char::len_utf8);
                }
                lines.push(rest[..cut].to_owned());
                rest = &rest[cut..];
            }
            line = rest.to_owned();
        }
        if !line.is_empty() || lines.len() == first {
            lines.push(line);
        }
    }
    lines
}

/// A box of `w` by `h` in the middle of `area`, no larger than it.
pub(crate) fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let (w, h) = (w.min(area.width), h.min(area.height));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

pub(crate) fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

/// The selection: reverse video, which reads on any terminal, coloured or not (spec §5.1).
pub(crate) fn selected() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

/// Core §13.5's severities: yellow and red with colour, bold without (spec §5.1).
pub(crate) fn severity_style(color: bool, severity: Severity) -> Style {
    match (severity, color) {
        (Severity::Normal, _) => Style::new(),
        (Severity::Warning, true) => Style::new().fg(Color::Yellow),
        (Severity::Critical, true) => Style::new().fg(Color::Red),
        (Severity::Warning | Severity::Critical, false) => bold(),
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use ratatui::style::Modifier;

    use super::*;
    use crate::actions::{ConfirmThen, Overlay};
    use crate::host::Action;
    use crate::msg::{Answer, Msg};
    use crate::testutil::*;
    use crate::update::update;

    #[test]
    fn the_header_shows_the_tabs_and_the_mode_and_the_footer_the_screens_keys() {
        let state = fresh(Screen::Accounts);
        let lines = draw(&state, 80, 24);
        // 43 columns of tabs, 14 of gap, and the mode and `? help` up to the last column.
        assert_eq!(
            lines[0],
            format!(
                " tagteam  [1 Accounts]  2 Tokens  3 History{}○ refresh 30s   ? help",
                " ".repeat(14)
            )
        );
        assert_eq!(
            lines[23],
            "s switch  b best  n next  e enable/disable  a alias  m move  x remove  r refresh"
        );
        let wide = draw(&state, 120, 40);
        assert!(wide[0].ends_with("○ refresh 30s   ? help"), "{}", wide[0]);
        assert_eq!(
            wide[39],
            "s switch  b best  n next  e enable/disable  a alias  m move  x remove  r refresh  R run  Enter history"
        );
        let history = draw(&fresh(Screen::History), 80, 24);
        assert_eq!(
            history[0],
            format!(
                " tagteam  1 Accounts  2 Tokens  [3 History]{}○ refresh 30s   ? help",
                " ".repeat(14)
            )
        );
        assert_eq!(history[23], "j/k account  [ ] range  r refresh  1 accounts");
        let tokens = draw(&fresh(Screen::Tokens), 80, 24);
        assert_eq!(
            tokens[0],
            format!(
                " tagteam  1 Accounts  [2 Tokens]  3 History{}○ refresh 30s   ? help",
                " ".repeat(14)
            )
        );
        assert_eq!(
            tokens[23],
            "p period  g group  f filter  [ ] range  Esc back  r refresh"
        );
    }

    #[test]
    fn below_80_by_24_only_the_notice_is_drawn() {
        let mut state = fresh(Screen::Accounts);
        state.toast("a toast", NOW_MS);
        for (w, h) in [(79, 24), (80, 23), (20, 5)] {
            let lines = draw(&state, w, h);
            let cut: String = TOO_SMALL.chars().take(usize::from(w)).collect();
            assert_eq!(lines[0], cut, "{w}x{h}");
            assert!(
                lines[1..].iter().all(String::is_empty),
                "{w}x{h}: {lines:?}"
            );
        }
    }

    #[test]
    fn the_footer_gives_way_to_a_toast_a_spinner_and_finishing() {
        let mut state = fresh(Screen::Accounts);
        state.running = Some(Action::AddLive { provider: cc() });
        assert_eq!(draw(&state, 80, 24)[23], "⠋ add…");
        let later = draw_at(&state, 80, 24, NOW_MS + 250);
        assert_eq!(later[23], "⠙ add…");
        state.toast("busy: add", NOW_MS);
        assert_eq!(draw(&state, 80, 24)[23], "busy: add");
        state.quitting = Some(0);
        assert_eq!(draw(&state, 80, 24)[23], "finishing add…");
        state.running = None;
        assert_eq!(draw(&state, 80, 24)[23], "finishing…");
    }

    #[test]
    fn a_toast_lasts_five_seconds() {
        let mut state = fresh(Screen::Accounts);
        state.toast("Switched", NOW_MS);
        update(&mut state, Msg::Tick, NOW_MS + 4_999);
        assert!(state.toast.is_some());
        state.dirty = false;
        update(&mut state, Msg::Tick, NOW_MS + 5_000);
        assert!(state.toast.is_none());
        assert!(state.dirty);
    }

    #[test]
    fn a_long_banner_wraps_onto_three_rows_at_most_and_the_body_gives_them_up() {
        // Spec §5.1: 80×24 is enough, so nothing a banner says may be lost at 80 columns.
        let mut state = fresh(Screen::Accounts);
        let words = "the store could not be opened because another tagteam holds its lock";
        state.banner = Some(format!("{words}; {words}"));
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[1..3],
            [
                " the store could not be opened because another tagteam holds its lock; the store",
                " could not be opened because another tagteam holds its lock",
            ]
        );
        assert_eq!(
            areas(&state, Rect::new(0, 0, 80, 24)).body,
            Rect::new(0, 3, 80, 20),
            "two rows, then the body"
        );
        state.banner = Some([words; 5].join("; "));
        let lines = draw(&state, 80, 24);
        assert!(lines[3].ends_with('…'), "{lines:?}");
        assert_eq!(width_of(&lines[3]), 80);
        assert_eq!(
            areas(&state, Rect::new(0, 0, 80, 24)).body,
            Rect::new(0, 4, 80, 19),
            "three rows at most"
        );
    }

    #[test]
    fn a_long_toast_wraps_upwards_from_the_footer() {
        let mut state = fresh(Screen::Accounts);
        let text = "Alias of 2 averyveryverylongname.with.a.long.tail@example.com set to the-most-descriptive-alias-anyone-could-want.";
        state.toast(text, NOW_MS);
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[22..],
            [
                "Alias of 2 averyveryverylongname.with.a.long.tail@example.com set to",
                "the-most-descriptive-alias-anyone-could-want.",
            ]
        );
        assert_eq!(
            areas(&state, Rect::new(0, 0, 80, 24)).body,
            Rect::new(0, 1, 80, 21),
            "the body gives up a row"
        );
    }

    #[test]
    fn an_error_banner_is_red_with_colour_and_bold_without() {
        for color in [true, false] {
            let mut state = fresh(Screen::Accounts);
            state.color = color;
            state.banner = Some("the store is locked".into());
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let lines = text_of(&buf);
            assert_eq!(lines[1], " the store is locked");
            let cell = &buf[(1, 1)];
            if color {
                assert_eq!(cell.fg, Color::Red);
                assert!(!cell.modifier.contains(Modifier::BOLD));
            } else {
                assert_eq!(cell.fg, Color::Reset);
                assert!(cell.modifier.contains(Modifier::BOLD));
            }
        }
    }

    #[test]
    fn text_is_cut_by_display_width_never_by_bytes() {
        assert_eq!(fit("山田太郎@x.co", 5), "山田…");
        assert_eq!(width_of(&fit("山田太郎@x.co", 4)), 3);
        assert_eq!(fit("😀😀😀", 4), "😀…");
        assert_eq!(fit("a@x.co", 6), "a@x.co");
        assert_eq!(fit("abc", 0), "");
        assert_eq!(pad("山田", 6), "山田  ");
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("山田太郎", 3), ["山", "田", "太", "郎"]);
        assert_eq!(wrap("山田", 1), ["山", "田"], "never an empty cut");
    }

    /// Review Focus 1: a dialog survives a resize below 80×24 and back, nothing panics, and its
    /// answer still reaches the lane waiting for it.
    #[test]
    fn a_dialog_survives_a_resize_below_the_minimum_and_back() {
        let (reply, answer) = std::sync::mpsc::channel::<Answer>();
        let mut state = fresh(Screen::Accounts);
        state.overlay = Some(Overlay::Confirm {
            question: "Remove 2 b@x.co?".into(),
            default_yes: false,
            choice: false,
            then: ConfirmThen::Reply(reply),
        });
        let drawn = draw(&state, 80, 24);
        assert!(
            drawn.iter().any(|l| l.contains("Remove 2 b@x.co?")),
            "{drawn:?}"
        );

        let mut terminal = terminal(80, 24);
        for (w, h) in [(60, 20), (10, 3), (1, 1), (80, 24)] {
            terminal.backend_mut().resize(w, h);
            update(
                &mut state,
                Msg::Resize {
                    width: w,
                    height: h,
                },
                NOW_MS,
            );
            terminal
                .draw(|frame| crate::view::view(&state, frame, NOW_MS))
                .unwrap();
            assert!(state.overlay.is_some(), "{w}x{h}");
        }
        let back = text_of(terminal.backend().buffer());
        assert!(
            back.iter().any(|l| l.contains("Remove 2 b@x.co?")),
            "{back:?}"
        );
        update(&mut state, key(KeyCode::Char('y')), NOW_MS);
        assert_eq!(answer.try_recv(), Ok(Answer::Confirm(true)));
        assert!(state.overlay.is_none());
    }

    #[test]
    fn flow_wraps_parts_under_the_indent_and_cuts_only_a_piece_too_wide() {
        let text = |lines: Vec<Line<'static>>| -> Vec<String> {
            lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .collect()
        };
        let head = vec![Span::raw(" ab   "), Span::raw("99%")];
        let parts = vec![
            "resets in 1h".to_owned(),
            "▲ ahead".to_owned(),
            "+1.0 pts/h · runs out in 2h (regression)".to_owned(),
        ];
        assert_eq!(
            text(flow(head.clone(), parts.clone(), 6, 30)),
            [
                " ab   99%  resets in 1h",
                "      ▲ ahead  +1.0 pts/h",
                "      runs out in 2h (regress…",
            ],
            "a part that fits nowhere whole breaks at its ` · `; only a piece too wide is cut"
        );
        assert_eq!(
            text(flow(head.clone(), parts.clone(), 6, 120)),
            [" ab   99%  resets in 1h  ▲ ahead  +1.0 pts/h · runs out in 2h (regression)"]
        );
        assert_eq!(
            text(flow(head, parts[2..].to_vec(), 6, 50)),
            [
                " ab   99%",
                "      +1.0 pts/h · runs out in 2h (regression)",
            ],
            "a part that fits a continuation line whole moves there whole"
        );
        assert_eq!(
            text(flow(
                vec![Span::raw("    ")],
                vec!["no rate yet".to_owned()],
                4,
                30
            )),
            ["    no rate yet"],
            "a line with nothing past the indent takes its first part unseparated"
        );
    }

    #[test]
    fn an_empty_toast_still_takes_the_footer_s_one_row() {
        let mut state = fresh(Screen::Accounts);
        state.toast("", NOW_MS);
        let a = areas(&state, Rect::new(0, 0, 80, 24));
        assert_eq!(a.footer.height, 1);
        assert_eq!(a.body.height, 24 - 1 - 1);
        assert_eq!(draw(&state, 80, 24)[23].trim_end(), "");
    }
}
