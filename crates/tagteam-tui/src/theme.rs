//! The one place a style is chosen (redesign spec): every other module asks the `Theme` for a
//! role's style and never names a colour or a modifier itself.

use ratatui::style::{Color, Modifier, Style};
use tagteam_engine::words::Severity;

/// What a span means, not how it looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Warn,
    Crit,
}

/// Whether the terminal's background is dark or light (spec §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Background {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    NoColor,
    Ansi,
    True(Background),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Theme {
    tier: Tier,
}

impl Theme {
    pub(crate) fn new(color: bool) -> Theme {
        Theme {
            tier: if color { Tier::Ansi } else { Tier::NoColor },
        }
    }

    /// Tier 2 on a terminal whose background is `background`; tier 0 stays tier 0 (spec §4.2).
    pub(crate) fn truecolor(self, background: Background) -> Theme {
        match self.tier {
            Tier::NoColor => self,
            Tier::Ansi | Tier::True(_) => Theme {
                tier: Tier::True(background),
            },
        }
    }

    /// Core §13.5's severities: yellow and red with colour, bold without (spec §5.1).
    pub(crate) fn role(self, role: Role) -> Style {
        match (self.tier, role) {
            (Tier::Ansi, Role::Warn) => Style::new().fg(Color::Yellow),
            (Tier::Ansi, Role::Crit) => Style::new().fg(Color::Red),
            (Tier::True(Background::Dark), Role::Warn) => {
                Style::new().fg(Color::Rgb(0xe5, 0xc0, 0x7b))
            }
            (Tier::True(Background::Light), Role::Warn) => {
                Style::new().fg(Color::Rgb(0x9a, 0x67, 0x00))
            }
            (Tier::True(Background::Dark), Role::Crit) => {
                Style::new().fg(Color::Rgb(0xf0, 0x71, 0x78))
            }
            (Tier::True(Background::Light), Role::Crit) => {
                Style::new().fg(Color::Rgb(0xcf, 0x22, 0x2e))
            }
            (Tier::NoColor, Role::Warn | Role::Crit) => self.strong(),
        }
    }

    pub(crate) fn severity(self, severity: Severity) -> Style {
        match severity {
            Severity::Normal => Style::new(),
            Severity::Warning => self.role(Role::Warn),
            Severity::Critical => self.role(Role::Crit),
        }
    }

    pub(crate) fn strong(self) -> Style {
        Style::new().add_modifier(Modifier::BOLD)
    }

    /// The selection: reverse video, which reads on any terminal, coloured or not (spec §5.1).
    pub(crate) fn selection(self) -> Style {
        Style::new().add_modifier(Modifier::REVERSED)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use ratatui::style::{Color, Modifier, Style};
    use tagteam_engine::words::Severity;

    use super::*;

    #[test]
    fn tier_1_and_no_colour_draw_today_s_styles() {
        let (ansi, plain) = (Theme::new(true), Theme::new(false));
        let bold = Style::new().add_modifier(Modifier::BOLD);
        assert_eq!(ansi.severity(Severity::Normal), Style::new());
        assert_eq!(
            ansi.severity(Severity::Warning),
            Style::new().fg(Color::Yellow)
        );
        assert_eq!(
            ansi.severity(Severity::Critical),
            Style::new().fg(Color::Red)
        );
        assert_eq!(plain.severity(Severity::Normal), Style::new());
        assert_eq!(plain.severity(Severity::Warning), bold);
        assert_eq!(plain.severity(Severity::Critical), bold);
        for theme in [ansi, plain] {
            assert_eq!(theme.strong(), bold);
            assert_eq!(
                theme.selection(),
                Style::new().add_modifier(Modifier::REVERSED)
            );
        }
    }

    #[test]
    fn tier_2_draws_warn_and_crit_in_its_own_hues() {
        let rgb = |r, g, b| Some(Color::Rgb(r, g, b));
        let dark = Theme::new(true).truecolor(Background::Dark);
        let light = Theme::new(true).truecolor(Background::Light);
        assert_eq!(dark.role(Role::Warn).fg, rgb(0xe5, 0xc0, 0x7b));
        assert_eq!(dark.role(Role::Crit).fg, rgb(0xf0, 0x71, 0x78));
        assert_eq!(light.role(Role::Warn).fg, rgb(0x9a, 0x67, 0x00));
        assert_eq!(light.role(Role::Crit).fg, rgb(0xcf, 0x22, 0x2e));
        for theme in [dark, light] {
            assert_eq!(theme.strong(), Theme::new(true).strong());
            assert_eq!(theme.selection(), Theme::new(true).selection());
        }
    }

    #[test]
    fn no_colour_stays_no_colour_whatever_the_background() {
        for background in [Background::Dark, Background::Light] {
            assert_eq!(Theme::new(false).truecolor(background), Theme::new(false));
        }
    }

    fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    fn names_word(line: &str) -> bool {
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        ["Color", "Modifier", "Stylize"].iter().any(|word| {
            line.match_indices(word).any(|(at, _)| {
                let before = line[..at].chars().next_back();
                let after = line[at + word.len()..].chars().next();
                !before.is_some_and(is_word) && !after.is_some_and(is_word)
            })
        })
    }

    #[test]
    fn no_module_but_the_theme_names_a_colour_or_a_modifier() {
        let src = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
        let mut files = Vec::new();
        rust_files(src, &mut files);
        let mut flagged = Vec::new();
        for path in files {
            let name = path.file_name().unwrap();
            if name == "theme.rs" || name == "testutil.rs" {
                continue;
            }
            let text = fs::read_to_string(&path).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            for (n, line) in lines.iter().enumerate() {
                let trimmed = line.trim();
                if trimmed == "#[cfg(test)]" {
                    let next = lines[n + 1..]
                        .iter()
                        .map(|l| l.trim())
                        .find(|l| !l.is_empty());
                    if next.is_some_and(|l| l.starts_with("mod ") && l.ends_with('{')) {
                        break;
                    }
                }
                if !trimmed.starts_with("//") && names_word(line) {
                    flagged.push(format!("{}:{}: {trimmed}", path.display(), n + 1));
                }
            }
        }
        assert!(flagged.is_empty(), "{flagged:#?}");
    }
}
