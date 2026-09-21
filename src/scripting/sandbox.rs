// chaosnexus-anvil/src/scripting/sandbox.rs
//
// OS-level sandbox hooks. See docs/operations/os_sandbox.md for deployment guidance.
//
// On Linux, Landlock (ABI V2+) restricts filesystem access after startup so that
// agent-driven I/O cannot escape the scripts tree. The shared library directory is
// `scripts_root/lib` (see `paths::lib_root`). Essential read-only system paths are
// also allowed so dynamic linking and TLS keep working — without those, HTTPS MCP
// and Cargo-built binaries break. Disable with `CHAOSNEXUS_ANVIL_DISABLE_LANDLOCK=1`
// (or legacy `CHAOSWRENCH_DISABLE_LANDLOCK=1`).

use std::path::{Path, PathBuf};

/// Result of attempting to apply the OS filesystem sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxStatus {
    /// Landlock rules installed for this process.
    Applied { scripts_root: PathBuf },
    /// Skipped via environment variable.
    DisabledByEnv,
    /// Not Linux (or not compiled with Landlock support).
    UnsupportedPlatform,
    /// Kernel/runtime does not support Landlock; process continues unsandboxed.
    Unavailable { reason: String },
    /// Ruleset failed; process continues unsandboxed (logged).
    Failed { reason: String },
}

/// Ensures the scripts tree layout exists (plugins, lib, pending, data, events).
pub fn ensure_scripts_layout(scripts_root: &Path) -> std::io::Result<()> {
    for sub in ["plugins", "lib", ".pending", "data", ".chaoswrench_data/events"] {
        std::fs::create_dir_all(scripts_root.join(sub))?;
    }
    Ok(())
}

/// Canonicalize `scripts_root` when possible; otherwise return an absolute path.
pub fn prepare_scripts_root(scripts_root: &Path) -> PathBuf {
    let _ = ensure_scripts_layout(scripts_root);
    scripts_root
        .canonicalize()
        .unwrap_or_else(|_| {
            if scripts_root.is_absolute() {
                scripts_root.to_path_buf()
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(scripts_root)
            }
        })
}

fn landlock_disabled_by_env() -> bool {
    for key in [
        "CHAOSNEXUS_ANVIL_DISABLE_LANDLOCK",
        "CHAOSWRENCH_DISABLE_LANDLOCK",
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

/// Applies Landlock filesystem restrictions when available.
///
/// Allowed with read+write: `scripts_root` (includes `plugins/`, `lib/`, `data/`,
/// `.pending/`, and engine event data under that tree).
///
/// Allowed read-only (best-effort): common system paths required for a working
/// process (`/usr`, `/lib`, `/lib64`, `/etc`, `/dev`, `/proc`, `/sys` read where present).
///
/// Does **not** grant access to the rest of `$HOME` — that is the agent jail.
pub fn apply_filesystem_sandbox_if_available(scripts_root: &Path) -> SandboxStatus {
    if landlock_disabled_by_env() {
        eprintln!("[chaosnexus-anvil] Landlock disabled via environment variable");
        return SandboxStatus::DisabledByEnv;
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = scripts_root;
        return SandboxStatus::UnsupportedPlatform;
    }

    #[cfg(target_os = "linux")]
    {
        apply_landlock_linux(scripts_root)
    }
}

#[cfg(target_os = "linux")]
fn apply_landlock_linux(scripts_root: &Path) -> SandboxStatus {
    use landlock::{
        Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
        RulesetCreatedAttr, ABI,
    };

    let root = prepare_scripts_root(scripts_root);

    // Prefer V2 (file refer) when available; fall back via BestEffort.
    let abi = ABI::V2;
    let access_rw = AccessFs::from_all(abi);
    let access_ro = AccessFs::from_read(abi);

    let ruleset = match Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(access_rw)
    {
        Ok(r) => r,
        Err(e) => {
            let reason = format!("handle_access: {e}");
            eprintln!("[chaosnexus-anvil] Landlock unavailable: {reason}");
            return SandboxStatus::Unavailable { reason };
        }
    };

    let created = match ruleset.create() {
        Ok(c) => c,
        Err(e) => {
            let reason = format!("create: {e}");
            eprintln!("[chaosnexus-anvil] Landlock unavailable: {reason}");
            return SandboxStatus::Unavailable { reason };
        }
    };

    // Primary jail: entire scripts root (plugins + lib + data + pending + events).
    let scripts_fd = match PathFd::new(&root) {
        Ok(fd) => fd,
        Err(e) => {
            let reason = format!("PathFd scripts_root {}: {e}", root.display());
            eprintln!("[chaosnexus-anvil] Landlock failed: {reason}");
            return SandboxStatus::Failed { reason };
        }
    };

    let mut created = match created.add_rule(PathBeneath::new(scripts_fd, access_rw)) {
        Ok(c) => c,
        Err(e) => {
            let reason = format!("add_rule scripts_root: {e}");
            eprintln!("[chaosnexus-anvil] Landlock failed: {reason}");
            return SandboxStatus::Failed { reason };
        }
    };

    // Optional PathBeneath rules. PathFd failures are skipped; add_rule errors abort
    // (RulesetCreated is moved and cannot be recovered on Err).
    let add_rw = |created: landlock::RulesetCreated, path: &Path, label: &str| {
        if !path.exists() {
            return Ok(created);
        }
        let fd = match PathFd::new(path) {
            Ok(fd) => fd,
            Err(e) => {
                eprintln!("[chaosnexus-anvil] Landlock: skip {label} PathFd: {e}");
                return Ok(created);
            }
        };
        created
            .add_rule(PathBeneath::new(fd, access_rw))
            .map_err(|e| format!("add_rule {label}: {e}"))
    };
    let add_ro = |created: landlock::RulesetCreated, path: &Path, label: &str| {
        if !path.exists() {
            return Ok(created);
        }
        let fd = match PathFd::new(path) {
            Ok(fd) => fd,
            Err(e) => {
                eprintln!("[chaosnexus-anvil] Landlock: skip {label} PathFd: {e}");
                return Ok(created);
            }
        };
        created
            .add_rule(PathBeneath::new(fd, access_ro))
            .map_err(|e| format!("add_rule {label}: {e}"))
    };

    // Explicit lib rule (documents shared-library jail; subset of scripts_root).
    let lib_path = root.join("lib");
    let _ = std::fs::create_dir_all(&lib_path);
    created = match add_rw(created, &lib_path, "lib") {
        Ok(c) => c,
        Err(reason) => {
            eprintln!("[chaosnexus-anvil] Landlock failed: {reason}");
            return SandboxStatus::Failed { reason };
        }
    };

    for p in [
        "/usr", "/lib", "/lib64", "/lib32", "/etc", "/dev", "/proc", "/sys",
        "/run/systemd/resolve",
    ] {
        created = match add_ro(created, Path::new(p), p) {
            Ok(c) => c,
            Err(reason) => {
                eprintln!("[chaosnexus-anvil] Landlock failed: {reason}");
                return SandboxStatus::Failed { reason };
            }
        };
    }

    for tmp in ["/tmp", "/var/tmp"] {
        created = match add_rw(created, Path::new(tmp), tmp) {
            Ok(c) => c,
            Err(reason) => {
                eprintln!("[chaosnexus-anvil] Landlock failed: {reason}");
                return SandboxStatus::Failed { reason };
            }
        };
    }

    match created.restrict_self() {
        Ok(status) => {
            eprintln!(
                "[chaosnexus-anvil] Landlock applied for scripts_root={} ({status:?})",
                root.display()
            );
            SandboxStatus::Applied {
                scripts_root: root,
            }
        }
        Err(e) => {
            let reason = format!("restrict_self: {e}");
            eprintln!("[chaosnexus-anvil] Landlock failed: {reason}");
            SandboxStatus::Failed { reason }
        }
    }
}

/// Logs a warning when the engine should not run with elevated privileges.
pub fn warn_if_privileged() {
    if std::env::var("CHAOSWRENCH_ALLOW_ROOT").is_ok()
        || std::env::var("CHAOSNEXUS_ANVIL_ALLOW_ROOT").is_ok()
    {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn prepare_scripts_root_creates_layout() {
        let root = std::env::temp_dir().join(format!(
            "anvil_landlock_layout_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let prepared = prepare_scripts_root(&root);
        assert!(prepared.join("plugins").is_dir());
        assert!(prepared.join("lib").is_dir());
        assert!(prepared.join(".pending").is_dir());
        assert!(prepared.join("data").is_dir());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[ignore = "applies process-wide Landlock; run alone: cargo test landlock_apply_smoke -- --ignored"]
    fn landlock_apply_smoke_does_not_panic() {
        // Applies Landlock when the kernel supports it; otherwise Unavailable/Failed is fine.
        let dir = std::env::temp_dir().join(format!(
            "chaosnexus-landlock-smoke-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let status = apply_filesystem_sandbox_if_available(&dir);
        match &status {
            SandboxStatus::Applied { scripts_root } => {
                assert!(scripts_root.join("lib").is_dir());
                assert!(scripts_root.join("plugins").is_dir());
            }
            SandboxStatus::Unavailable { reason } | SandboxStatus::Failed { reason } => {
                assert!(!reason.is_empty());
            }
            SandboxStatus::DisabledByEnv | SandboxStatus::UnsupportedPlatform => {}
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disable_env_skips_sandbox() {
        // SAFETY: test process; serial enough for this crate's unit tests.
        unsafe {
            std::env::set_var("CHAOSNEXUS_ANVIL_DISABLE_LANDLOCK", "1");
        }
        let root = std::env::temp_dir().join(format!(
            "anvil_landlock_skip_{}",
            std::process::id()
        ));
        let _ = fs::create_dir_all(&root);
        let status = apply_filesystem_sandbox_if_available(&root);
        unsafe {
            std::env::remove_var("CHAOSNEXUS_ANVIL_DISABLE_LANDLOCK");
        }
        assert_eq!(status, SandboxStatus::DisabledByEnv);
        let _ = fs::remove_dir_all(&root);
    }
}
