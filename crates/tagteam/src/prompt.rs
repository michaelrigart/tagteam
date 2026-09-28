use std::io::{BufRead, IsTerminal, Write};

pub trait Prompter {
    /// True only when a person can answer: stdin and stderr are both terminals.
    fn interactive(&self) -> bool;
    fn confirm(&mut self, question: &str, default_yes: bool) -> bool;
    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize>;
    fn secret(&mut self, question: &str) -> Option<String>;
}

pub struct TtyPrompter;

fn read_line() -> String {
    let mut s = String::new();
    let _ = std::io::stdin().lock().read_line(&mut s);
    s.trim().to_owned()
}

impl Prompter for TtyPrompter {
    fn interactive(&self) -> bool {
        std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
    }

    fn confirm(&mut self, question: &str, default_yes: bool) -> bool {
        eprint!(
            "{question} {} ",
            if default_yes { "[Y/n]" } else { "[y/N]" }
        );
        let _ = std::io::stderr().flush();
        match read_line().to_ascii_lowercase().as_str() {
            "" => default_yes,
            a => a == "y" || a == "yes",
        }
    }

    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize> {
        for (i, o) in options.iter().enumerate() {
            eprintln!("  {}) {o}", i + 1);
        }
        eprint!("{question} [1-{}] ", options.len());
        let _ = std::io::stderr().flush();
        read_line()
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=options.len()).contains(n))
            .map(|n| n - 1)
    }

    fn secret(&mut self, question: &str) -> Option<String> {
        rpassword::prompt_password(question).ok()
    }
}
