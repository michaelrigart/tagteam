//! §12.7's opt-in shell wrapper: one function per registered provider that supports sessions,
//! named after its launch command. The function always runs `tagteam run`, which decides from
//! the mappings and `exec`s the plain launch command where none applies (§12.1). Where
//! `tagteam` itself is not on `PATH`, it runs the launch command directly.

use crate::cli::ShellArg;

/// A provider the wrapper covers: its ID, which `--provider` takes, and its launch command,
/// which names the function and runs when `tagteam` cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Wrapped {
    pub(crate) id: String,
    pub(crate) launch: &'static str,
}

/// The text `shell-init` prints: every function in `providers`' order, a blank line between
/// two. zsh and bash get the same POSIX function, which `sh` runs too:
/// - `command -v` is a builtin in each, and tests `PATH` without running anything;
/// - `command <launch>` skips the function itself, so the fallback never recurses;
/// - `"$@"` passes every argument as it was given, the empty ones included.
///
/// A user's alias of the same name makes the definition fail when the rc file runs, rather
/// than shadow the wrapper silently. fish's `$argv` is a list, and expands one argument per
/// element without splitting.
pub(crate) fn script(shell: ShellArg, providers: &[Wrapped]) -> String {
    let functions: Vec<String> = providers.iter().map(|p| function(shell, p)).collect();
    functions.join("\n")
}

fn function(shell: ShellArg, p: &Wrapped) -> String {
    let (id, launch) = (p.id.as_str(), p.launch);
    match shell {
        ShellArg::Zsh | ShellArg::Bash => format!(
            r#"{launch}() {{
  if command -v tagteam >/dev/null 2>&1; then
    tagteam run --provider {id} -- "$@"
  else
    command {launch} "$@"
  fi
}}
"#
        ),
        ShellArg::Fish => format!(
            r#"function {launch} --description '{launch} through tagteam run'
    if command -q tagteam
        tagteam run --provider {id} -- $argv
    else
        command {launch} $argv
    end
end
"#
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude() -> Wrapped {
        Wrapped {
            id: "claude-code".into(),
            launch: "claude",
        }
    }

    const POSIX: &str = "claude() {
  if command -v tagteam >/dev/null 2>&1; then
    tagteam run --provider claude-code -- \"$@\"
  else
    command claude \"$@\"
  fi
}
";

    #[test]
    fn zsh_gets_a_posix_function_that_runs_tagteam_run() {
        assert_eq!(script(ShellArg::Zsh, &[claude()]), POSIX);
    }

    #[test]
    fn bash_gets_the_same_function() {
        assert_eq!(script(ShellArg::Bash, &[claude()]), POSIX);
    }

    #[test]
    fn fish_gets_a_function_over_argv() {
        assert_eq!(
            script(ShellArg::Fish, &[claude()]),
            "function claude --description 'claude through tagteam run'
    if command -q tagteam
        tagteam run --provider claude-code -- $argv
    else
        command claude $argv
    end
end
"
        );
    }

    #[test]
    fn each_provider_with_sessions_gets_its_own_function() {
        let fake = Wrapped {
            id: "fake-agent".into(),
            launch: "fakeagent",
        };
        let text = script(ShellArg::Bash, &[claude(), fake]);
        assert_eq!(
            text,
            format!(
                "{POSIX}\nfakeagent() {{
  if command -v tagteam >/dev/null 2>&1; then
    tagteam run --provider fake-agent -- \"$@\"
  else
    command fakeagent \"$@\"
  fi
}}
"
            )
        );
        assert_eq!(script(ShellArg::Zsh, &[]), "", "no provider, no function");
    }
}
