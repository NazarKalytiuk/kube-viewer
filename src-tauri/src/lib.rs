//! KubeViewer backend.

pub mod cluster;
pub mod commands;
pub mod error;
pub mod kubeconfig;
pub mod resources;
pub mod shell_env;
pub mod subscriptions;
pub mod view;

use tracing_subscriber::EnvFilter;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("KUBEVIEWER_LOG")
                .unwrap_or_else(|_| EnvFilter::new("kube_viewer_lib=info,warn")),
        )
        .init();

    tauri::Builder::default()
        .manage(commands::AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::resource_kinds,
            commands::list_contexts,
            commands::connect,
            commands::list_namespaces,
            commands::open_view,
            commands::set_view_query,
            commands::get_rows,
            commands::row_index,
            commands::open_detail,
            commands::open_logs,
            commands::close_subscription,
            commands::reset_subscriptions,
            commands::reveal_secret,
        ])
        .run(tauri::generate_context!())
        .expect("error while running KubeViewer");
}
