//! Connected clusters (one cached client per kubeconfig context).

use std::collections::HashMap;
use std::time::Duration;

use k8s_openapi::api::core::v1::Namespace;
use kube::api::ListParams;
use kube::{Api, Client, ResourceExt};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::error::{AppError, AppResult, is_forbidden};
use crate::kubeconfig;
use crate::shell_env::login_env;

/// Exec plugins may need to refresh a token (SSO, OIDC) on first request.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceList {
    pub names: Vec<String>,
    /// The user may not list namespaces (RBAC); `names` then holds the
    /// context's default namespace so namespace-scoped users still work.
    pub forbidden: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterInfo {
    pub context: String,
    pub server_version: String,
    /// Namespace configured for the context, if any.
    pub default_namespace: Option<String>,
    pub namespaces: NamespaceList,
}

struct Connection {
    client: Client,
    default_namespace: Option<String>,
}

#[derive(Default)]
pub struct Clusters {
    connections: Mutex<HashMap<String, Connection>>,
}

impl Clusters {
    /// (Re)connects to a context. The kubeconfig is re-read every time so
    /// edits made in a terminal are picked up without restarting the app.
    pub async fn connect(&self, context: &str) -> AppResult<ClusterInfo> {
        let login = login_env().await;
        let (cfg, _) = kubeconfig::load(login)?;
        let default_namespace = cfg
            .contexts
            .iter()
            .find(|c| c.name == context)
            .and_then(|c| c.context.as_ref())
            .and_then(|c| c.namespace.clone())
            .filter(|n| !n.is_empty());

        let config = kubeconfig::client_config(cfg, context, login).await?;
        let client = Client::try_from(config).map_err(|e| AppError::Connect(e.to_string()))?;
        let version = tokio::time::timeout(CONNECT_TIMEOUT, client.apiserver_version())
            .await
            .map_err(|_| {
                AppError::Connect(format!(
                    "API server did not answer within {}s",
                    CONNECT_TIMEOUT.as_secs()
                ))
            })??;

        let namespaces = list_namespaces(&client, default_namespace.as_deref()).await?;
        self.connections.lock().await.insert(
            context.to_string(),
            Connection {
                client,
                default_namespace: default_namespace.clone(),
            },
        );
        Ok(ClusterInfo {
            context: context.to_string(),
            server_version: version.git_version,
            default_namespace,
            namespaces,
        })
    }

    pub async fn client(&self, context: &str) -> AppResult<Client> {
        self.connections
            .lock()
            .await
            .get(context)
            .map(|c| c.client.clone())
            .ok_or_else(|| {
                AppError::InvalidRequest(format!("context `{context}` is not connected"))
            })
    }

    pub async fn namespaces(&self, context: &str) -> AppResult<NamespaceList> {
        let (client, default_namespace) = {
            let connections = self.connections.lock().await;
            let conn = connections.get(context).ok_or_else(|| {
                AppError::InvalidRequest(format!("context `{context}` is not connected"))
            })?;
            (conn.client.clone(), conn.default_namespace.clone())
        };
        list_namespaces(&client, default_namespace.as_deref()).await
    }
}

async fn list_namespaces(client: &Client, fallback: Option<&str>) -> AppResult<NamespaceList> {
    let api: Api<Namespace> = Api::all(client.clone());
    match api.list_metadata(&ListParams::default()).await {
        Ok(list) => {
            let mut names: Vec<String> = list.items.iter().map(|n| n.name_any()).collect();
            names.sort();
            Ok(NamespaceList {
                names,
                forbidden: false,
            })
        }
        Err(err) if is_forbidden(&err) => Ok(NamespaceList {
            names: vec![fallback.unwrap_or("default").to_string()],
            forbidden: true,
        }),
        Err(err) => Err(err.into()),
    }
}
