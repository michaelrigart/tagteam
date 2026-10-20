use std::ffi::OsStr;

use tagteam_core::ProviderId;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::{Engine, EngineError};

use crate::cli::{CompletionShell, ShellArg};
use crate::shell_init::Wrapped;
use crate::{shell_init, statusline};

use super::Ended;
use super::context::{ColorEnv, Context, Io};
use super::failure::{EXIT_USAGE, Failure, KIND_UNSUPPORTED, KIND_USAGE};

const STATUSLINE_UNDER_JSON: &str = "statusline prints a line of text; run it without --json";
const SHELL_INIT_UNDER_JSON: &str = "shell-init prints shell code; run it without --json";
const COMPLETIONS_UNDER_JSON: &str = "completions prints a script; run it without --json";

/// §13.5's fast path, taken before `build_engine`: no lock check, no settings warnings (a status
/// bar has nowhere to show them), and an engine walled off from the Keychain and the network
/// (`statusline::engine`), which detects the run shell itself and resolves the provider in
/// Decision 13's order. An unreadable marker shows nothing and exits 0 (§12.8). Once the line
/// is out, it records its session as evidence for the token report (token report spec §6.3).
/// `main_with_args` has already drained stdin.
pub(super) fn run_statusline(
    ctx: Context,
    io: &mut Io<'_>,
    json: bool,
    color: ColorEnv,
    provider: Option<String>,
    print_config: bool,
) -> i32 {
    if json {
        io.fail(true, KIND_USAGE, STATUSLINE_UNDER_JSON);
        return EXIT_USAGE;
    }
    let (engine, provider) = statusline::engine(ctx, provider.as_deref());
    let session_id = engine
        .provider(&provider)
        .ok()
        .and_then(|p| p.vars().session_id)
        .and_then(|name| engine.env().var(name))
        .and_then(OsStr::to_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned);
    let result = statusline_supported(&engine, &provider).and_then(|()| {
        if print_config {
            let p = engine.provider(&provider)?;
            let (hint, snippet) =
                statusline::print_config(p.as_ref(), engine.env()).ok_or_else(|| {
                    Failure::Message(
                        KIND_UNSUPPORTED,
                        format!("{} has no statusline", p.display_name()),
                    )
                })?;
            let _ = writeln!(io.err, "{hint}");
            return Ok(snippet);
        }
        let view = engine.statusline(&provider)?;
        let settings = engine.settings();
        // The line goes to Claude Code, which renders ANSI colour but is never a terminal, so
        // `auto` colours it: the same rule as `list`, with the terminal test taken as met.
        let colour = color.enabled(settings.color);
        Ok(statusline::line(
            &view,
            &settings.statusline_format,
            engine.now_ms() / 1000,
            colour,
        ))
    });
    match result {
        Ok(text) => {
            let _ = write!(io.out, "{text}");
            if !print_config {
                let _ = io.out.flush();
                statusline::record_session(&engine, &provider, session_id.as_deref());
            }
            0
        }
        Err(failure) => match failure.report(io, false, false) {
            Ended::Code(code) => code,
            Ended::Interrupted(_) | Ended::Child(_) => unreachable!("no signals, no child"),
        },
    }
}

/// The TUI runs only from the binary, on its terminal (`tui_cmd`); in-process there is none to
/// hand over, so the answer is a usage refusal.
pub(super) fn refuse_tui(io: &mut Io<'_>, json: bool) -> Ended {
    let message = if json {
        crate::tui_cmd::TUI_UNDER_JSON
    } else {
        crate::tui_cmd::NEEDS_TERMINAL
    };
    io.fail(json, KIND_USAGE, message);
    Ended::Code(EXIT_USAGE)
}

/// §13.7's `completions`: the script, or a usage refusal under `--json`; a script that cannot be
/// written is an `io` error, except to a reader that closed the pipe early.
pub(super) fn answer_completions(io: &mut Io<'_>, json: bool, shell: CompletionShell) -> Ended {
    if json {
        io.fail(true, KIND_USAGE, COMPLETIONS_UNDER_JSON);
        return Ended::Code(EXIT_USAGE);
    }
    let written = io
        .out
        .write_all(&shell.script())
        .and_then(|()| io.out.flush());
    match written {
        Ok(()) => Ended::Code(0),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ended::Code(0),
        Err(e) => Failure::from(EngineError::from(e)).report(io, json, true),
    }
}

/// §12.7's `shell-init`, answered before the engine is built: one wrapper per registered
/// provider with sessions (`Wrapped`). `--provider` must name a registered provider, as for
/// every command.
pub(super) fn run_shell_init(
    registry: &ProviderRegistry,
    io: &mut Io<'_>,
    json: bool,
    provider: Option<&str>,
    shell: ShellArg,
) -> i32 {
    if json {
        io.fail(true, KIND_USAGE, SHELL_INIT_UNDER_JSON);
        return EXIT_USAGE;
    }
    if let Some(id) = provider {
        if registry.get(&ProviderId::new(id)).is_none() {
            let e = EngineError::UnknownProvider(id.to_owned());
            return io.fail(false, e.kind(), &e.to_string());
        }
    }
    let wrapped: Vec<Wrapped> = registry
        .all()
        .iter()
        .filter(|p| p.capabilities().sessions)
        .map(|p| Wrapped {
            id: p.id().as_str().to_owned(),
            launch: p.launch_command(),
        })
        .collect();
    let _ = write!(io.out, "{}", shell_init::script(shell, &wrapped));
    0
}

/// §4.5: `statusline` refuses for a provider without the capability.
fn statusline_supported(engine: &Engine, provider: &ProviderId) -> Result<(), Failure> {
    let p = engine.provider(provider)?;
    match statusline::unsupported(p.capabilities(), p.display_name()) {
        Some(message) => Err(Failure::Message(KIND_UNSUPPORTED, message)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use crate::prompt::Prompter;

    use super::super::failure::EXIT_ERROR;
    use super::*;

    struct NoPrompts;

    impl Prompter for NoPrompts {
        fn interactive(&self) -> bool {
            false
        }
        fn confirm(&mut self, question: &str, _default_yes: bool) -> bool {
            panic!("unexpected prompt: {question}");
        }
        fn choose(&mut self, question: &str, _options: &[String]) -> Option<usize> {
            panic!("unexpected prompt: {question}");
        }
        fn secret(&mut self, question: &str) -> Option<String> {
            panic!("unexpected prompt: {question}");
        }
    }

    /// Fails the write, or only the flush, with `kind` and `message`.
    struct Unwritable {
        kind: io::ErrorKind,
        message: &'static str,
        at_flush: bool,
    }

    impl Unwritable {
        fn error(&self) -> io::Error {
            io::Error::new(self.kind, self.message)
        }
    }

    impl io::Write for Unwritable {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.at_flush {
                Ok(buf.len())
            } else {
                Err(self.error())
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.at_flush {
                Err(self.error())
            } else {
                Ok(())
            }
        }
    }

    fn complete_into(out: &mut Unwritable) -> (Ended, String) {
        let mut err = Vec::new();
        let ended = answer_completions(
            &mut Io {
                out,
                err: &mut err,
                prompter: &mut NoPrompts,
            },
            false,
            CompletionShell::Bash,
        );
        (ended, String::from_utf8(err).unwrap())
    }

    #[test]
    fn a_completions_script_that_cannot_be_written_is_an_io_error() {
        let mut out = Unwritable {
            kind: io::ErrorKind::Other,
            message: "disk full",
            at_flush: false,
        };
        let (ended, err) = complete_into(&mut out);
        assert!(matches!(ended, Ended::Code(EXIT_ERROR)));
        let expected = EngineError::from(io::Error::other("disk full"));
        assert_eq!(err, format!("tagteam: {expected}\n"));
    }

    #[test]
    fn a_completions_script_that_cannot_be_flushed_is_an_io_error() {
        let mut out = Unwritable {
            kind: io::ErrorKind::Other,
            message: "disk full",
            at_flush: true,
        };
        let (ended, err) = complete_into(&mut out);
        assert!(matches!(ended, Ended::Code(EXIT_ERROR)));
        let expected = EngineError::from(io::Error::other("disk full"));
        assert_eq!(err, format!("tagteam: {expected}\n"));
    }

    #[test]
    fn a_completions_script_whose_reader_left_is_not_an_error() {
        for at_flush in [false, true] {
            let mut out = Unwritable {
                kind: io::ErrorKind::BrokenPipe,
                message: "reader closed",
                at_flush,
            };
            let (ended, err) = complete_into(&mut out);
            assert!(matches!(ended, Ended::Code(0)));
            assert!(err.is_empty(), "{err:?}");
        }
    }
}
