//! What orzma injects into a shell so it reports its working directory.

/// What orzma adds to a shell's spawn so the shell reports its working
/// directory through `OSC 9;9`.
// NOTE: only `build_shell_command`'s Windows arm consumes this, so a
// Unix release build sees no caller. CI runs `clippy --all-targets -D
// warnings`, which fails on the resulting `dead_code` unless the
// expectation below is present.
#[cfg_attr(
    all(unix, not(test)),
    expect(
        dead_code,
        reason = "the Windows integration is unit-tested on every platform"
    )
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShellIntegration {
    /// Arguments appended to the shell's own command line.
    Args(Vec<String>),
    /// An environment variable the shell builds its prompt from.
    Env {
        /// The variable's name.
        key: String,
        /// The variable's value.
        value: String,
    },
    /// The shell is not recognized, so nothing is added.
    None,
}

impl ShellIntegration {
    /// What to inject into `shell`, given the `PROMPT` this process
    /// inherited.
    ///
    /// A shell whose executable stem is not one orzma recognizes gets
    /// nothing, so a wrapper script or an unknown shell is spawned
    /// exactly as it would be without this feature.
    pub(crate) fn for_shell(shell: &str, inherited_prompt: Option<&str>) -> Self {
        match ShellKind::of(shell) {
            Some(ShellKind::PowerShell) => Self::Args(vec![
                "-NoExit".to_string(),
                "-Command".to_string(),
                POWERSHELL_PROMPT_HOOK.to_string(),
            ]),
            Some(ShellKind::Cmd) => Self::Env {
                key: "PROMPT".to_string(),
                value: format!(
                    r"$e]9;9;$P$e\{}",
                    inherited_prompt.filter(|p| !p.is_empty()).unwrap_or("$P$G")
                ),
            },
            None => Self::None,
        }
    }
}

/// A shell orzma knows how to make report its working directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellKind {
    /// Windows PowerShell or PowerShell 7.
    PowerShell,
    /// The Windows command interpreter.
    Cmd,
}

impl ShellKind {
    /// The shell `shell` names, or `None` when orzma does not recognize
    /// it.
    ///
    /// Only the executable stem is inspected, so a directory, an
    /// extension, and letter case do not matter. Both `/` and `\` are
    /// read as directory separators, whatever the host platform.
    fn of(shell: &str) -> Option<Self> {
        let name = shell.rsplit(['/', '\\']).next()?;
        let stem = name.rsplit_once('.').map_or(name, |(before, _)| before);
        match stem.to_ascii_lowercase().as_str() {
            "pwsh" | "powershell" => Some(Self::PowerShell),
            "cmd" => Some(Self::Cmd),
            _ => None,
        }
    }
}

/// The prompt hook injected into PowerShell.
///
/// The hook calls back whatever `prompt` was defined before it, and
/// writes `OSC 9;9` with the location's filesystem path only while the
/// current location is on the `FileSystem` provider.
// NOTE: the snippet contains no double quote, so it crosses
// `CommandBuilder`'s Windows command-line quoting and PowerShell's own
// re-parse as one plainly-quoted argument with no `\"` escapes. The
// emitted sequence still carries the quotes Microsoft's published form
// uses, built from `[char]34`.
const POWERSHELL_PROMPT_HOOK: &str = concat!(
    "$global:__orzmaInnerPrompt = $function:prompt; ",
    "function global:prompt { ",
    "if ($PWD.Provider.Name -eq 'FileSystem') { ",
    "[Console]::Write((",
    "[char]27, ']9;9;', [char]34, $PWD.ProviderPath, [char]34, [char]7",
    ") -join '') ",
    "}; ",
    "& $global:__orzmaInnerPrompt ",
    "}",
);

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that PowerShell is recognized by stem alone, whatever
    /// case, extension, or directory the configured shell carries.
    ///
    /// Case: one user leaves the shell unset so it resolves to the bare
    /// name `pwsh`, and another points it at an installed
    /// `C:\Program Files\PowerShell\7\pwsh.exe`.
    #[test]
    fn powershell_is_recognized_by_its_stem() {
        for shell in [
            "pwsh",
            "powershell",
            "POWERSHELL.EXE",
            r"C:\Program Files\PowerShell\7\pwsh.exe",
        ] {
            assert!(
                matches!(
                    ShellIntegration::for_shell(shell, None),
                    ShellIntegration::Args(_)
                ),
                "{shell} was not recognized as PowerShell"
            );
        }
    }

    /// Asserts that the PowerShell arm keeps the shell interactive and
    /// passes the hook as a literal command.
    ///
    /// Case: orzma spawns the user's PowerShell for a new pane and needs
    /// it to land at a prompt with the hook installed.
    #[test]
    fn the_powershell_arm_keeps_the_shell_interactive() {
        let ShellIntegration::Args(args) = ShellIntegration::for_shell("pwsh", None) else {
            panic!("pwsh must produce arguments");
        };
        assert_eq!(args[0], "-NoExit");
        assert_eq!(args[1], "-Command");
        assert!(args[2].contains("$function:prompt"));
        assert!(args[2].contains("FileSystem"));
        assert!(args[2].contains("ProviderPath"));
    }

    /// Asserts that the cmd arm prefixes the inherited prompt rather
    /// than replacing it.
    ///
    /// Case: a user has customized `PROMPT` with `setx`, and orzma must
    /// add its report without discarding that customization.
    #[test]
    fn the_cmd_arm_prefixes_the_inherited_prompt() {
        let ShellIntegration::Env { key, value } =
            ShellIntegration::for_shell("cmd.exe", Some("$T$G"))
        else {
            panic!("cmd must produce an environment variable");
        };
        assert_eq!(key, "PROMPT");
        assert_eq!(value, r"$e]9;9;$P$e\$T$G");
    }

    /// Asserts that the cmd arm falls back to the stock prompt when the
    /// environment carries none.
    ///
    /// Case: orzma is launched from a session where `PROMPT` was never
    /// set, so cmd would otherwise use its built-in default.
    #[test]
    fn the_cmd_arm_falls_back_to_the_stock_prompt() {
        let ShellIntegration::Env { value, .. } = ShellIntegration::for_shell("cmd", None) else {
            panic!("cmd must produce an environment variable");
        };
        assert_eq!(value, r"$e]9;9;$P$e\$P$G");
    }

    /// Asserts that a shell orzma does not recognize is left untouched.
    ///
    /// Case: a user configures `nu`, a login wrapper script, or a shell
    /// orzma has never heard of, and none of them may have arguments or
    /// environment variables added behind their back.
    #[test]
    fn an_unrecognized_shell_is_left_alone() {
        for shell in ["nu", "/bin/zsh", "bash", "git-bash.exe", ""] {
            assert!(
                matches!(
                    ShellIntegration::for_shell(shell, None),
                    ShellIntegration::None
                ),
                "{shell} must not be touched"
            );
        }
    }
}
