use std::io::{BufRead, IsTerminal, Write};

pub trait Prompter {
    /// True only when a person can answer: stdin and stderr are both terminals.
    fn interactive(&self) -> bool;
    fn confirm(&mut self, question: &str, default_yes: bool) -> bool;
    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize>;
    fn secret(&mut self, question: &str) -> Option<String>;
}

pub struct TtyPrompter;

/// Writes a prompt to stderr, so stdout stays the command's output. A failed write is
/// ignored rather than panicking: the answer read next decides either way.
fn ask(text: &str) {
    let mut err = std::io::stderr().lock();
    let _ = write!(err, "{text}");
    let _ = err.flush();
}

/// One answer, trimmed. `None` at end of input (Ctrl-D) or on a read error: every prompt takes
/// that as a decline, never as the default answer an empty line (Enter) gives.
fn read_answer(input: &mut dyn BufRead) -> Option<String> {
    let mut s = String::new();
    match input.read_line(&mut s) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(s.trim().to_owned()),
    }
}

fn confirmed(answer: Option<String>, default_yes: bool) -> bool {
    match answer.map(|a| a.to_ascii_lowercase()).as_deref() {
        None => false,
        Some("") => default_yes,
        Some(a) => a == "y" || a == "yes",
    }
}

/// The 0-based index of a 1-based answer within `count` options.
fn chosen(answer: Option<String>, count: usize) -> Option<usize> {
    answer?
        .parse::<usize>()
        .ok()
        .filter(|n| (1..=count).contains(n))
        .map(|n| n - 1)
}

impl Prompter for TtyPrompter {
    fn interactive(&self) -> bool {
        std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
    }

    fn confirm(&mut self, question: &str, default_yes: bool) -> bool {
        let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
        ask(&format!("{question} {hint} "));
        confirmed(read_answer(&mut std::io::stdin().lock()), default_yes)
    }

    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize> {
        let mut text: String = options
            .iter()
            .enumerate()
            .map(|(i, o)| format!("  {}) {o}\n", i + 1))
            .collect();
        text.push_str(&format!("{question} [1-{}] ", options.len()));
        ask(&text);
        chosen(read_answer(&mut std::io::stdin().lock()), options.len())
    }

    fn secret(&mut self, question: &str) -> Option<String> {
        rpassword::prompt_password(question).ok()
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Cursor, Read};

    use super::*;

    fn answer(input: &str) -> Option<String> {
        read_answer(&mut Cursor::new(input.as_bytes()))
    }

    #[test]
    fn ctrl_d_at_a_yes_by_default_prompt_declines() {
        // Ctrl-D on an empty line: end of input, with nothing read.
        assert_eq!(answer(""), None);
        assert!(!confirmed(answer(""), true));
        // Enter is an empty line, and takes the default.
        assert!(confirmed(answer("\n"), true));
        assert!(!confirmed(answer("\n"), false));
        assert!(confirmed(answer(" Yes\n"), false));
        assert!(!confirmed(answer("n\n"), true));
    }

    #[test]
    fn a_read_error_declines() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("the terminal went away"))
            }
        }
        let broken = read_answer(&mut BufReader::new(Broken));
        assert_eq!(broken, None);
        assert!(!confirmed(broken, true));
    }

    #[test]
    fn a_choice_is_one_based_and_in_range() {
        assert_eq!(chosen(answer("2\n"), 2), Some(1));
        assert_eq!(chosen(answer("3\n"), 2), None);
        assert_eq!(chosen(answer("0\n"), 2), None);
        assert_eq!(chosen(answer(""), 2), None);
    }
}
