// chaosnexus-anvil/src/scripting/shell_exec.rs
//
// Cross-platform shell invocation for `run_command`. POSIX shells use `-c`;
// PowerShell uses `-NoProfile -NonInteractive -Command` so Windows plugins can
// rely on `powershell` instead of `cmd.exe`.

use std::path::Path;
use std::process::{Command, Output};

/// How a shell executable expects its inline command argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    /// POSIX-compatible shells (sh, bash, zsh, fish).
    Posix,
    /// PowerShell (pwsh, powershell.exe).
    PowerShell,
    /// Windows `cmd.exe`.
    Cmd,
}

/// Classifies `shell` by executable basename (e.g. `pwsh`, `powershell`, `sh`).
pub fn shell_invocation_kind(shell: &str) -> ShellKind {
    let base = Path::new(shell)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(shell)
        .to_lowercase();
    let stem = base.strip_suffix(".exe").unwrap_or(&base);
    match stem {
        "powershell" | "pwsh" => ShellKind::PowerShell,
        "cmd" => ShellKind::Cmd,
        _ => ShellKind::Posix,
    }
}

/// Runs `command` through `shell` using the correct argument convention.
pub fn run_shell(shell: &str, command: &str) -> Result<Output, std::io::Error> {
    let mut cmd = Command::new(shell);
    match shell_invocation_kind(shell) {
        ShellKind::PowerShell => {
            cmd.args(["-NoProfile", "-NonInteractive", "-Command", command]);
        }
        ShellKind::Cmd => {
            cmd.args(["/c", command]);
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
}
