// chaosnexus-anvil/src/scripting/sandbox.rs
//
// OS-level sandbox hooks. See docs/operations/os_sandbox.md for deployment guidance.

/// Placeholder for future Landlock rule application on Linux.
pub fn apply_filesystem_sandbox_if_available(_scripts_root: &std::path::Path) {
    // Landlock requires Linux 5.13+ and explicit rule setup at startup.
    // Deployment docs describe systemd/container isolation until automated here.
}

/// Logs a warning when the engine should not run with elevated privileges.
pub fn warn_if_privileged() {
    if std::env::var("CHAOSWRENCH_ALLOW_ROOT").is_ok() {
        return;
    }
    #[cfg(target_os = "linux")]
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("Uid:") {
                if rest.split_whitespace().next() == Some("0") {
                    eprintln!(
                        "[chaosnexus-anvil] WARNING: running as root. Use an unprivileged service user for production."
                    );
                }
                break;
            }
        }
    }
}
