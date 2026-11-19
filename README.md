# KubeViewer

A fast, read-only Kubernetes cluster viewer for macOS. Tauri 2 + Rust + Solid.
It is built to stay smooth on very large clusters: 100k-object views load in about a second and scroll without lag.

## Features

- Contexts from `KUBECONFIG` / `~/.kube/config`. When launched from Finder, the login shell's `KUBECONFIG` is also picked up.
- Auth: client certificates, tokens, basic, OIDC/GCP auth-providers and **exec plugins** (`aws`, `gke-gcloud-auth-plugin`, `kubelogin`, …). Exec plugins are resolved through the login-shell `PATH`, so they work outside a terminal.
- **TLS is always verified** unless the kubeconfig explicitly sets `insecure-skip-tls-verify`. The sidebar warns when it does.
- 13 kinds: Pods, Deployments, ReplicaSets, StatefulSets, DaemonSets, Services, Ingresses, ConfigMaps, Secrets, PVCs, Events, Nodes and Namespaces. Columns follow `kubectl get`.
- Live updates via watch. Re-lists after `410 Gone` replace the data atomically, so deletions are never missed.
- Rows are keyed by UID, so same-named objects in different namespaces never collide.
- Backend sorting (natural order: `pod-2` < `pod-10`) and filtering on any text column.
- Detail panel:
  - **Status:** type-specific fields and conditions.
  - **Labels:** labels and annotations.
  - **YAML:** CodeMirror with search and folding; `managedFields` is stripped.
  - A button to expand the panel to full width.
- **Logs** (built for heavy use):
  - **Storage:** logs live in Rust with a 256 MB budget per view; only visible lines reach the UI. 1M lines: filter ~16 ms, regex ~35 ms, find-next under 1 µs.
  - **Time:** timestamps are always fetched and parsed once. **No time / Local / UTC** switches instantly, without reloading.
  - **History:** last 100 / 1k / 10k lines, last 5 min / 1 h / 24 h, or everything. A **Previous** toggle shows the crashed container instance.
  - **All containers:** one view, merged by timestamp, with a colored container tag on each line.
  - **Live follow:** resumes after container restarts and dropped connections with no gaps or duplicates (resumes from the last timestamp). Init containers and finished pods stop cleanly.
  - **Search:** **Find** (highlight, ↑/↓, Enter / ⇧Enter, "3 / 120") or **Filter** (matching lines only); regex and match-case options; ⌘F.
  - **Reading:** ANSI colors, ERROR/WARN highlighting, line numbers, wrap toggle.
  - **Auto-scroll:** pauses when you scroll up; a "↓ N new lines" pill brings you back.
  - **Download:**
    - *Full log from cluster* streams straight to disk (constant memory, merged by time across containers, progress and cancel).
    - *Loaded lines* saves exactly what is shown, filter included.
    - Both offer UTC / local / no time and an optional `[container]` prefix, write atomically (`.partial` then rename), and offer *Show in Finder*.
- Secrets are masked everywhere, including the `last-applied-configuration` annotation. Values are fetched only when you click **Reveal values**.
- RBAC-friendly. If listing namespaces is forbidden, the app falls back to the context's namespace and lets you type one. Errors never lock you out of switching contexts.

## Architecture

```
Rust (src-tauri)                               Solid (src)
──────────────────────────────────────────     ─────────────────────────────────
kube::runtime::watcher ─► ViewStore            RowWindow: cache of visible rows
  (rows only, never full objects;               + overscan, one request in flight
   sorted order kept incrementally,             ResourceTable: TanStack Virtual,
   O(log n) per event)                          fixed 26px rows, ~40 DOM rows
        │  every 100 ms if changed                        ▲
        └── Channel: {version,total} ───────────►  get_rows(id, start, end)
```

- **Data stays in Rust.** Only the visible window (~60–120 rows) crosses IPC.
- **Change notifications are coalesced** into one update per 100 ms (detail: 250 ms, logs: 50 ms batches of up to 2,000 lines). Cluster churn therefore never floods the UI.
- **Every subscription is a task in a registry.** It is closed when the component unmounts or the context changes, and on webview reload (`reset_subscriptions`).
- **Measured on a real cluster** (kind, release build, 100k ConfigMaps):

  | Measurement | Time |
  |---|---|
  | First rows | 100 ms |
  | Full list | 1.2 s |
  | Window read | ~6 µs |
  | Re-sort + filter | ~1 ms |

## Development

Requirements: Rust (stable, edition 2024), Node 22+, pnpm 10.

```sh
pnpm install
pnpm tauri:dev        # run the app
pnpm check            # typecheck + vitest + rustfmt + clippy + cargo test
pnpm e2e              # Playwright (WebKit) against a mocked backend with 100k pods
pnpm tauri:build      # ad-hoc signed .app in src-tauri/target/release/bundle/macos
pnpm tauri:dmg        # .dmg (macOS asks once to let the terminal control Finder)
```

Integration tests run against a real cluster. They only create and delete `kv-it-*` namespaces:

```sh
kind create cluster --name kubeviewer-test
KUBEVIEWER_IT=1 cargo test --manifest-path src-tauri/Cargo.toml --test cluster -- --test-threads=1
# perf (creates objects; use KUBEVIEWER_PERF_COUNT=100000 for 100k)
KUBEVIEWER_IT=1 cargo test --release --manifest-path src-tauri/Cargo.toml --test cluster -- --ignored --nocapture
```

Logging: `KUBEVIEWER_LOG=kube_viewer_lib=debug pnpm tauri:dev`.

## Not yet

- Write actions (scale, restart, delete, edit)
- `exec` into pods
- Port-forward
- CRDs
- Metrics
- Signed/notarized releases
