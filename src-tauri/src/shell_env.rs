//! Resolves the user's login-shell environment.
//!
//! Apps launched from Finder/Dock inherit launchd's minimal environment
//! (`PATH=/usr/bin:/bin:/usr/sbin:/sbin`, no `KUBECONFIG`). Exec credential
//! plugins (`aws`, `gke-gcloud-auth-plugin`, `kubelogin`, …) and custom
//! kubeconfig locations live in the shell profile, so we ask the login shell
//! once and use its `PATH` / `KUBECONFIG` without mutating our own process
//! environment.

use std::time::Duration;

use tokio::process::Command;
use tokio::sync::OnceCell;

const MARKER: &str = "__KUBEVIEWER_ENV__";
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoginEnv {
    pub path: Option<String>,
    pub kubeconfig: Option<String>,
}

static LOGIN_ENV: OnceCell<LoginEnv> = OnceCell::const_new();

/// Returns the login-shell environment, probing it on first use.
/// Never fails: on any problem the corresponding field is `None`.
pub async fn login_env() -> &'static LoginEnv {
    LOGIN_ENV.get_or_init(probe).await
}

async fn probe() -> LoginEnv {
    let Some(shell) = std::env::var_os("SHELL").filter(|s| !s.is_empty()) else {
        return LoginEnv::default();
    };
    // `-i -l` loads both profile and rc files, where PATH is usually extended.
    // Output is fenced by markers because rc files may print banners.
    let script = format!(r#"printf '{MARKER}%s{MARKER}%s{MARKER}' "$PATH" "${{KUBECONFIG-}}""#);
    let mut cmd = Command::new(shell);
    cmd.args(["-i", "-l", "-c", &script])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);

    match tokio::time::timeout(PROBE_TIMEOUT, cmd.output()).await {
        Ok(Ok(output)) => parse_probe_output(&String::from_utf8_lossy(&output.stdout)),
        Ok(Err(err)) => {
            tracing::warn!("login shell probe failed: {err}");
            LoginEnv::default()
        }
        Err(_) => {
            tracing::warn!("login shell probe timed out after {PROBE_TIMEOUT:?}");
            LoginEnv::default()
        }
    }
}

/// Extracts the fenced values from the probe output.
pub fn parse_probe_output(stdout: &str) -> LoginEnv {
    let mut parts = stdout.split(MARKER);
    // Anything before the first marker is shell noise.
    let _noise = parts.next();
    let (Some(path), Some(kubeconfig), Some(_)) = (parts.next(), parts.next(), parts.next()) else {
        return LoginEnv::default();
    };
    let non_empty = |s: &str| (!s.trim().is_empty()).then(|| s.to_string());
    LoginEnv {
        path: non_empty(path),
        kubeconfig: non_empty(kubeconfig),
    }
}

/// Combines the login-shell PATH with this process' PATH (login entries first,
/// duplicates removed), so tools resolve the same way as in the user's terminal.
pub fn merged_path(login_path: Option<&str>, process_path: Option<&str>) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    let merged: Vec<&str> = login_path
        .into_iter()
        .chain(process_path)
        .flat_map(|p| p.split(':'))
        .filter(|entry| !entry.is_empty() && seen.insert(*entry))
        .collect();
    (!merged.is_empty()).then(|| merged.join(":"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_values_after_shell_noise() {
        let out = format!(
            "Welcome!\n{MARKER}/opt/homebrew/bin:/usr/bin{MARKER}/Users/me/.kube/a:/Users/me/.kube/b{MARKER}"
        );
        let env = parse_probe_output(&out);
        assert_eq!(env.path.as_deref(), Some("/opt/homebrew/bin:/usr/bin"));
        assert_eq!(
            env.kubeconfig.as_deref(),
            Some("/Users/me/.kube/a:/Users/me/.kube/b")
        );
    }

    #[test]
    fn empty_kubeconfig_is_none() {
        let out = format!("{MARKER}/usr/bin{MARKER}{MARKER}");
        assert_eq!(parse_probe_output(&out).kubeconfig, None);
    }

    #[test]
    fn truncated_output_yields_default() {
        assert_eq!(parse_probe_output("no markers here"), LoginEnv::default());
        assert_eq!(
            parse_probe_output(&format!("{MARKER}/usr/bin")),
            LoginEnv::default()
        );
    }

    #[test]
    fn merges_paths_login_first_without_duplicates() {
        let merged = merged_path(Some("/opt/homebrew/bin:/usr/bin"), Some("/usr/bin:/bin"));
        assert_eq!(merged.as_deref(), Some("/opt/homebrew/bin:/usr/bin:/bin"));
        assert_eq!(merged_path(None, None), None);
    }
}
