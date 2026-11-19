# KubeViewer: notes for agents

Tauri 2 + Rust (`src-tauri`) + Solid 1.9 + Tailwind 4 (`src`). macOS first. Read-only Kubernetes viewer.

## Rules

- **Large data must stay fast.** Never ship full object lists to the webview.
  - Table data lives in `view/store.rs` (`ViewStore`); the frontend pulls windows via `get_rows`.
  - New lists must be virtualized (TanStack Virtual, fixed row height).
- **Stay on Solid 1.9.x** until `vite-plugin-solid`, TanStack Virtual and Kobalte support Solid 2.
- **Subscriptions** (`open_view`, `open_detail`, `open_logs`):
  - They are tasks in `subscriptions.rs`.
  - The frontend must call `close()` in `onCleanup`.
  - Backend tasks must end when their sink returns `false`.
- **Adding a resource kind takes four changes:**
  1. a variant in `ResourceKind`, plus its columns;
  2. a `with_kind!` arm;
  3. `RowSource` and `DetailSource` impls;
  4. the e2e mock, if the UI depends on it.

  `every_kind_matches_its_column_count` and `kind_scope_matches_type_scope` guard consistency.
- **Secrets:** values must never appear in rows, detail, YAML or logs. Only `reveal_secret` returns them, on explicit user action.
- **TLS:** never default `insecure_skip_tls_verify` to true.
- **Errors** cross IPC as `{ code, message }` (`error.rs`). Map new failure modes to a code.

## Checks

- `pnpm check` runs typecheck, vitest, rustfmt, clippy `-D warnings` and cargo tests.
- `pnpm e2e` runs Playwright on WebKit with a mocked backend (`tests/e2e/mockBackend.ts`).
- Real-cluster tests need the `kind-kubeviewer-test` context:

  ```sh
  KUBEVIEWER_IT=1 cargo test --manifest-path src-tauri/Cargo.toml --test cluster -- --test-threads=1
  ```
