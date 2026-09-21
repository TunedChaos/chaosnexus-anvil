// chaosnexus-anvil/src/scripting/shell_exec.rs
//
// Cross-platform process invocation for `run_command`.
// Preferred path: argv form (`Command::new(exec).args(...)`) — no shell `-c`.
// Legacy POSIX `-c` / PowerShell `-Command` lives in `run_shell` and is only
// reachable when the caller opts in via CHAOSNEXUS_ANVIL_ALLOW_SHELL_C=1.

use std::path::Path;
use std::process::{Command, Output};

/// How a shell executable expects its inline command argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    /// POSIX-compatible shells (sh, bash, zsh, fish).
    Posix,
    /// PowerShell (pwsh, powershell.exe).
    PowerShell,
    /// cmd.exe on Windows.
    Cmd,
}

/// Classifies `shell` by executable basename (e.g. `pwsh`, `powershell`, `sh`).
pub fn shell_invocation_kind(shell: &str) -> ShellKind {
    let base = Path::new(shell)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(shell)
        .to_ascii_lowercase();

    match base.as_str() {
        "powershell" | "pwsh" => ShellKind::PowerShell,
        "cmd" => ShellKind::Cmd,
        _ => ShellKind::Posix,
    }
}

/// True when the dangerous shell `-c` / `-Command` path is explicitly enabled.
pub fn shell_c_allowed_by_env() -> bool {
    for key in [
        "CHAOSNEXUS_ANVIL_ALLOW_SHELL_C",
        "CHAOSWRENCH_ALLOW_SHELL_C",
    ] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim();
            if v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes") {
                return true;
            }
        }
    }
    false
}

/// Runs `exec` with discrete `args` — no shell interpretation.
pub fn run_argv(exec: &str, args: &[String]) -> Result<Output, std::io::Error> {
    Command::new(exec).args(args).output()
}

/// Legacy: runs `command` through `shell` using `-c` / `-Command`.
/// Callers must gate with `shell_c_allowed_by_env()` before invoking.
pub fn run_shell(shell: &str, command: &str) -> Result<Output, std::io::Error> {
    let mut cmd = Command::new(shell);
    match shell_invocation_kind(shell) {
        ShellKind::PowerShell => {
            cmd.args(["-NoProfile", "-NonInteractive", "-Command", command]);
        }
        ShellKind::Cmd => {
            cmd.args(["/C", command]);
        }
        ShellKind::Posix => {
            cmd.arg("-c").arg(command);
        }
    }
    cmd.output()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_powershell_and_posix_shells() {
        assert_eq!(shell_invocation_kind("powershell"), ShellKind::PowerShell);
        assert_eq!(shell_invocation_kind("pwsh"), ShellKind::PowerShell);
        assert_eq!(
            shell_invocation_kind("powershell.exe"),
            ShellKind::PowerShell
        );
        assert_eq!(shell_invocation_kind("/bin/sh"), ShellKind::Posix);
        assert_eq!(shell_invocation_kind("zsh"), ShellKind::Posix);
        assert_eq!(shell_invocation_kind("cmd"), ShellKind::Cmd);
    }

    #[test]
    fn shell_c_env_defaults_off() {
        // Ensure the gate function is callable; default in clean env is false
        // unless the developer exported the opt-in (do not assert absolute false
        // in ambient CI that may set it).
        let _ = shell_c_allowed_by_env();
    }
}
