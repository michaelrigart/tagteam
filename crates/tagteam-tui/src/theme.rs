//! The one place a style is chosen (redesign spec): every other module asks the `Theme` for a
//! role's style and never names a colour or a modifier itself.

use ratatui::style::{Color, Modifier, Style};
use tagteam_engine::words::Severity;

/// What a span means, not how it looks (spec §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    /// Values and names.
    Text,
    /// Labels, units, countdowns, secondary columns, key labels and hints.
    Muted,
    /// Rules, tracks, separators and borders; never text that must be read.
    Faint,
    /// The current tab, the selection, `●`, key letters and normal fills.
    Accent,
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
    pub(crate) const fn new(color: bool) -> Theme {
        Theme {
            tier: if color { Tier::Ansi } else { Tier::NoColor },
        }
    }

    /// Tier 2 on a terminal whose background is `background`; tier 0 stays tier 0 (spec §4.2).
    pub(crate) const fn truecolor(self, background: Background) -> Theme {
        match self.tier {
            Tier::NoColor => self,
            Tier::Ansi | Tier::True(_) => Theme {
                tier: Tier::True(background),
            },
        }
    }

    /// §4.1's style for `role` in this tier: tier 0 by modifiers alone, tier 1 by the terminal's
    /// own ANSI colours, tier 2 by the background's hues.
    pub(crate) fn role(self, role: Role) -> Style {
        match self.tier {
            Tier::NoColor => match role {
                Role::Text => Style::new(),
                Role::Muted | Role::Faint => Style::new().add_modifier(Modifier::DIM),
                Role::Accent | Role::Warn => Style::new().add_modifier(Modifier::BOLD),
                Role::Crit => Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            },
            Tier::Ansi => match role {
                Role::Text => Style::new(),
                Role::Muted => Style::new().add_modifier(Modifier::DIM),
                Role::Faint => Style::new().fg(Color::DarkGray),
                Role::Accent => Style::new().fg(Color::Cyan),
                Role::Warn => Style::new().fg(Color::Yellow),
                Role::Crit => Style::new().fg(Color::Red),
            },
            Tier::True(background) => {
                let (dark, light) = match role {
                    Role::Text => return Style::new(),
                    Role::Muted => ((0x9d, 0xa5, 0xb4), (0x57, 0x60, 0x6a)),
                    Role::Faint => ((0x4b, 0x52, 0x63), (0xc4, 0xc9, 0xcf)),
                    Role::Accent => ((0x56, 0xb6, 0xc2), (0x0a, 0x7d, 0x8c)),
                    Role::Warn => ((0xe5, 0xc0, 0x7b), (0x9a, 0x67, 0x00)),
                    Role::Crit => ((0xf0, 0x71, 0x78), (0xcf, 0x22, 0x2e)),
                };
                let (r, g, b) = match background {
                    Background::Dark => dark,
                    Background::Light => light,
                };
                Style::new().fg(Color::Rgb(r, g, b))
            }
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
    use crate::testutil::TIERS;

    #[test]
    fn every_role_has_section_4_1_s_style_in_every_tier() {
        // §4.1's table, row by row, in `TIERS`' order: tier 0, tier 1, tier 2 dark and light.
        let none = Style::new();
        let modifier = |m| Style::new().add_modifier(m);
        let fg = |c| Style::new().fg(c);
        let rgb = |r, g, b| Style::new().fg(Color::Rgb(r, g, b));
        let table = [
            (Role::Text, [none, none, none, none]),
            (
                Role::Muted,
                [
                    modifier(Modifier::DIM),
                    modifier(Modifier::DIM),
                    rgb(0x9d, 0xa5, 0xb4),
                    rgb(0x57, 0x60, 0x6a),
                ],
            ),
            (
                Role::Faint,
                [
                    modifier(Modifier::DIM),
                    fg(Color::DarkGray),
                    rgb(0x4b, 0x52, 0x63),
                    rgb(0xc4, 0xc9, 0xcf),
                ],
            ),
            (
                Role::Accent,
                [
                    modifier(Modifier::BOLD),
                    fg(Color::Cyan),
                    rgb(0x56, 0xb6, 0xc2),
                    rgb(0x0a, 0x7d, 0x8c),
                ],
            ),
            (
                Role::Warn,
                [
                    modifier(Modifier::BOLD),
                    fg(Color::Yellow),
                    rgb(0xe5, 0xc0, 0x7b),
                    rgb(0x9a, 0x67, 0x00),
                ],
            ),
            (
                Role::Crit,
                [
                    modifier(Modifier::BOLD | Modifier::UNDERLINED),
                    fg(Color::Red),
                    rgb(0xf0, 0x71, 0x78),
                    rgb(0xcf, 0x22, 0x2e),
                ],
            ),
        ];
        for (role, styles) in table {
            for (theme, style) in TIERS.into_iter().zip(styles) {
                assert_eq!(theme.role(role), style, "{role:?} in {theme:?}");
            }
        }
        let bold = modifier(Modifier::BOLD);
        for theme in TIERS {
            assert_eq!(theme.severity(Severity::Normal), none, "{theme:?}");
            assert_eq!(theme.severity(Severity::Warning), theme.role(Role::Warn));
            assert_eq!(theme.severity(Severity::Critical), theme.role(Role::Crit));
            assert_eq!(theme.strong(), bold, "{theme:?}");
            assert_eq!(theme.selection(), modifier(Modifier::REVERSED));
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
