//! kubeconfig loading, context listing and per-context client configuration.

use std::collections::HashMap;
use std::path::PathBuf;

use kube::config::{AuthInfo, KubeConfigOptions, Kubeconfig};
use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::shell_env::{LoginEnv, merged_path};

/// How a context authenticates. Shown in the context picker so users know
/// which external tool a context depends on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthKind {
    Exec { command: String },
    AuthProvider { name: String },
    Token,
    ClientCertificate,
    Basic,
    None,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextInfo {
    pub name: String,
    pub cluster: String,
    pub user: Option<String>,
    pub namespace: Option<String>,
    pub server: Option<String>,
    pub auth: AuthKind,
    /// True when the kubeconfig explicitly disables TLS verification.
    pub insecure_skip_tls_verify: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextList {
    pub contexts: Vec<ContextInfo>,
    pub current: Option<String>,
    /// Files the configuration was merged from, for display/debugging.
    pub sources: Vec<String>,
}

/// Loads the kubeconfig the same way `kubectl` would in the user's terminal:
/// process `KUBECONFIG`, then the login shell's `KUBECONFIG`, then `~/.kube/config`.
pub fn load(login: &LoginEnv) -> AppResult<(Kubeconfig, Vec<PathBuf>)> {
    let raw = std::env::var("KUBECONFIG")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| login.kubeconfig.clone());

    let paths: Vec<PathBuf> = match raw {
        Some(value) => std::env::split_paths(&value)
            .filter(|p| !p.as_os_str().is_empty())
            .collect(),
        None => vec![default_path()?],
    };

    let mut merged: Option<Kubeconfig> = None;
    for path in &paths {
        let cfg = Kubeconfig::read_from(path)
            .map_err(|e| AppError::Kubeconfig(format!("{}: {e}", path.display())))?;
        merged = Some(match merged {
            None => cfg,
            Some(acc) => acc
                .merge(cfg)
                .map_err(|e| AppError::Kubeconfig(e.to_string()))?,
        });
    }
    let cfg = merged.ok_or_else(|| AppError::Kubeconfig("no kubeconfig files found".into()))?;
    Ok((cfg, paths))
}

fn default_path() -> AppResult<PathBuf> {
    let home =
        std::env::var_os("HOME").ok_or_else(|| AppError::Kubeconfig("HOME is not set".into()))?;
    Ok(PathBuf::from(home).join(".kube").join("config"))
}

pub fn list_contexts(cfg: &Kubeconfig, sources: &[PathBuf]) -> ContextList {
    let clusters: HashMap<&str, &kube::config::Cluster> = cfg
        .clusters
        .iter()
        .filter_map(|c| c.cluster.as_ref().map(|cl| (c.name.as_str(), cl)))
        .collect();
    let users: HashMap<&str, &AuthInfo> = cfg
        .auth_infos
        .iter()
        .filter_map(|u| u.auth_info.as_ref().map(|a| (u.name.as_str(), a)))
        .collect();

    let mut contexts: Vec<ContextInfo> = cfg
        .contexts
        .iter()
        .filter_map(|named| {
            let ctx = named.context.as_ref()?;
            let cluster = clusters.get(ctx.cluster.as_str());
            let auth = ctx
                .user
                .as_deref()
                .and_then(|u| users.get(u))
                .map(|a| auth_kind(a))
                .unwrap_or(AuthKind::None);
            Some(ContextInfo {
                name: named.name.clone(),
                cluster: ctx.cluster.clone(),
                user: ctx.user.clone(),
                namespace: ctx.namespace.clone().filter(|n| !n.is_empty()),
                server: cluster.and_then(|c| c.server.clone()),
                auth,
                insecure_skip_tls_verify: cluster
                    .and_then(|c| c.insecure_skip_tls_verify)
                    .unwrap_or(false),
            })
        })
        .collect();
    contexts.sort_by(|a, b| a.name.cmp(&b.name));

    ContextList {
        contexts,
        current: cfg.current_context.clone().filter(|c| !c.is_empty()),
        sources: sources.iter().map(|p| p.display().to_string()).collect(),
    }
}

fn auth_kind(auth: &AuthInfo) -> AuthKind {
    if let Some(exec) = &auth.exec {
        return AuthKind::Exec {
            command: exec.command.clone().unwrap_or_default(),
        };
    }
    if let Some(provider) = &auth.auth_provider {
        return AuthKind::AuthProvider {
            name: provider.name.clone(),
        };
    }
    if auth.token.is_some() || auth.token_file.is_some() {
        return AuthKind::Token;
    }
    if auth.client_certificate.is_some() || auth.client_certificate_data.is_some() {
        return AuthKind::ClientCertificate;
    }
    if auth.username.is_some() {
        return AuthKind::Basic;
    }
    AuthKind::None
}

/// Gives every exec credential plugin the login-shell PATH unless the
/// kubeconfig already pins one. Rust's `Command` resolves the program through
/// the child's PATH when it is set explicitly, so this fixes plugin lookup
/// without touching the process-wide environment.
pub fn with_exec_path(mut cfg: Kubeconfig, path: Option<&str>) -> Kubeconfig {
    let Some(path) = path else { return cfg };
    for named in &mut cfg.auth_infos {
        let Some(exec) = named.auth_info.as_mut().and_then(|a| a.exec.as_mut()) else {
            continue;
        };
        let env = exec.env.get_or_insert_with(Vec::new);
        let has_path = env
            .iter()
            .any(|e| e.get("name").map(String::as_str) == Some("PATH"));
        if !has_path {
            env.push(HashMap::from([
                ("name".to_string(), "PATH".to_string()),
                ("value".to_string(), path.to_string()),
            ]));
        }
    }
    cfg
}

/// Builds a client configuration for one context.
pub async fn client_config(
    cfg: Kubeconfig,
    context: &str,
    login: &LoginEnv,
) -> AppResult<kube::Config> {
    if !cfg.contexts.iter().any(|c| c.name == context) {
        return Err(AppError::InvalidRequest(format!(
            "context `{context}` not found in kubeconfig"
        )));
    }
    let path = merged_path(login.path.as_deref(), std::env::var("PATH").ok().as_deref());
    let cfg = with_exec_path(cfg, path.as_deref());
    let options = KubeConfigOptions {
        context: Some(context.to_string()),
        ..Default::default()
    };
    kube::Config::from_custom_kubeconfig(cfg, &options)
        .await
        .map_err(|e| AppError::Kubeconfig(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
apiVersion: v1
kind: Config
current-context: kind-dev
clusters:
  - name: kind-dev
    cluster:
      server: https://127.0.0.1:6443
      certificate-authority-data: ZmFrZQ==
  - name: lab
    cluster:
      server: https://lab.example:6443
      insecure-skip-tls-verify: true
users:
  - name: kind-dev
    user:
      client-certificate-data: ZmFrZQ==
      client-key-data: ZmFrZQ==
  - name: eks
    user:
      exec:
        apiVersion: client.authentication.k8s.io/v1beta1
        command: aws
        args: ["eks", "get-token", "--cluster-name", "prod"]
  - name: pinned
    user:
      exec:
        apiVersion: client.authentication.k8s.io/v1beta1
        command: kubelogin
        env:
          - name: PATH
            value: /custom/bin
contexts:
  - name: kind-dev
    context: { cluster: kind-dev, user: kind-dev }
  - name: prod
    context: { cluster: lab, user: eks, namespace: payments }
  - name: pinned
    context: { cluster: lab, user: pinned }
"#;

    fn sample() -> Kubeconfig {
        Kubeconfig::from_yaml(SAMPLE).unwrap()
    }

    #[test]
    fn lists_contexts_with_auth_kind_and_tls_flag() {
        let list = list_contexts(&sample(), &[]);
        assert_eq!(list.current.as_deref(), Some("kind-dev"));
        let names: Vec<_> = list.contexts.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["kind-dev", "pinned", "prod"]);

        let kind = &list.contexts[0];
        assert_eq!(kind.auth, AuthKind::ClientCertificate);
        assert!(
            !kind.insecure_skip_tls_verify,
            "TLS must be verified unless explicitly disabled"
        );

        let prod = &list.contexts[2];
        assert_eq!(
            prod.auth,
            AuthKind::Exec {
                command: "aws".into()
            }
        );
        assert_eq!(prod.namespace.as_deref(), Some("payments"));
        assert!(prod.insecure_skip_tls_verify);
    }

    fn exec_path(cfg: &Kubeconfig, user: &str) -> Option<String> {
        let auth = cfg
            .auth_infos
            .iter()
            .find(|a| a.name == user)?
            .auth_info
            .as_ref()?;
        auth.exec
            .as_ref()?
            .env
            .as_ref()?
            .iter()
            .find(|e| e.get("name").map(String::as_str) == Some("PATH"))
            .and_then(|e| e.get("value").cloned())
    }

    #[test]
    fn injects_path_into_exec_plugins_but_keeps_pinned_path() {
        let cfg = with_exec_path(sample(), Some("/opt/homebrew/bin:/usr/bin"));
        assert_eq!(
            exec_path(&cfg, "eks").as_deref(),
            Some("/opt/homebrew/bin:/usr/bin")
        );
        assert_eq!(exec_path(&cfg, "pinned").as_deref(), Some("/custom/bin"));
        let cert_user = cfg
            .auth_infos
            .iter()
            .find(|a| a.name == "kind-dev")
            .unwrap();
        assert!(cert_user.auth_info.as_ref().unwrap().exec.is_none());
    }

    #[tokio::test]
    async fn rejects_unknown_context() {
        let err = client_config(sample(), "missing", &LoginEnv::default())
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::InvalidRequest(_)));
    }
}
