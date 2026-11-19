// In-page fake of the Tauri backend, injected with `page.addInitScript`.
// It implements `window.__TAURI_INTERNALS__` (invoke + channel callbacks)
// so the real frontend runs unmodified in a browser.

export interface MockOptions {
  podCount: number;
}

// This function is serialized into the page: it must be self-contained.
export function installMockBackend(options: MockOptions) {
  type Row = { uid: string; name: string; namespace: string | null; cells: unknown[] };
  type View = { rows: Row[]; order: Row[]; sort: { column: string; descending: boolean }; filter: string; version: number; columns: { id: string }[] };

  const callbacks = new Map<number, (msg: unknown) => void>();
  let nextCallback = 1;
  const channelIndex = new Map<number, number>();
  const views = new Map<number, View>();
  let nextSub = 1;
  const now = Math.floor(Date.now() / 1000);

  const send = (channel: { id: number }, message: unknown) => {
    const index = channelIndex.get(channel.id) ?? 0;
    channelIndex.set(channel.id, index + 1);
    setTimeout(() => callbacks.get(channel.id)?.({ index, message }), 0);
  };

  const kinds = [
    { kind: "pods", title: "Pods", group: "Workloads", namespaced: true },
    { kind: "deployments", title: "Deployments", group: "Workloads", namespaced: true },
    { kind: "configMaps", title: "ConfigMaps", group: "Config & Storage", namespaced: true },
    { kind: "nodes", title: "Nodes", group: "Cluster", namespaced: false },
  ];

  const podColumns = [
    { id: "name", title: "Name", kind: "text", width: 320 },
    { id: "namespace", title: "Namespace", kind: "text", width: 160 },
    { id: "ready", title: "Ready", kind: "text", width: 70 },
    { id: "status", title: "Status", kind: "status", width: 150 },
    { id: "restarts", title: "Restarts", kind: "number", width: 80 },
    { id: "age", title: "Age", kind: "age", width: 70 },
  ];

  function makePods(): Row[] {
    const rows: Row[] = [];
    for (let i = 0; i < options.podCount; i++) {
      const name = `pod-${String(i).padStart(6, "0")}`;
      const ns = `team-${i % 20}`;
      const crash = i % 97 === 0;
      rows.push({
        uid: `uid-${i}`,
        name,
        namespace: ns,
        cells: [name, ns, crash ? "0/1" : "1/1", crash ? { t: "CrashLoopBackOff", tone: "error" } : { t: "Running", tone: "ok" }, crash ? 12 : 0, now - i * 7],
      });
    }
    return rows;
  }

  function cmp(a: unknown, b: unknown): number {
    const va = a !== null && typeof a === "object" ? (a as { t: string }).t : a;
    const vb = b !== null && typeof b === "object" ? (b as { t: string }).t : b;
    if (typeof va === "number" && typeof vb === "number") return va - vb;
    return String(va ?? "").localeCompare(String(vb ?? ""));
  }

  function refresh(view: View) {
    const col = Math.max(0, view.columns.findIndex((c) => c.id === view.sort.column));
    const needle = view.filter.trim().toLowerCase();
    const filtered = needle ? view.rows.filter((r) => `${r.name} ${r.namespace}`.toLowerCase().includes(needle)) : view.rows.slice();
    filtered.sort((a, b) => cmp(a.cells[col], b.cells[col]) * (view.sort.descending ? -1 : 1) || a.name.localeCompare(b.name));
    view.order = filtered;
    view.version += 1;
  }

  const handlers: Record<string, (args: Record<string, any>) => unknown> = {
    resource_kinds: () => kinds,
    reset_subscriptions: () => null,
    list_contexts: () => ({
      current: "kind-dev",
      sources: ["/Users/test/.kube/config"],
      contexts: [
        { name: "kind-dev", cluster: "kind-dev", user: "kind-dev", namespace: null, server: "https://127.0.0.1:6443", auth: { type: "client_certificate" }, insecureSkipTlsVerify: false },
        { name: "prod-eks", cluster: "prod", user: "eks", namespace: "payments", server: "https://prod.example", auth: { type: "exec", command: "aws" }, insecureSkipTlsVerify: false },
      ],
    }),
    connect: ({ context }) => {
      if (context === "prod-eks") throw { code: "unauthorized", message: "exec plugin: token expired" };
      return { context, serverVersion: "v1.36.1", defaultNamespace: null, namespaces: { names: ["default", "kube-system"], forbidden: false } };
    },
    list_namespaces: () => ({ names: ["default", "kube-system"], forbidden: false }),
    open_view: ({ kind, sort, filter, onUpdate }) => {
      const rows = kind === "pods" ? makePods() : [];
      const view: View = { rows, order: [], sort: sort ?? { column: "name", descending: false }, filter: filter ?? "", version: 0, columns: podColumns };
      refresh(view);
      const id = nextSub++;
      views.set(id, view);
      send(onUpdate, { version: view.version, total: view.order.length, unfiltered: rows.length, phase: "ready", error: null });
      return { id, columns: podColumns, sort: view.sort };
    },
    set_view_query: ({ id, sort, filter }) => {
      const view = views.get(id)!;
      view.sort = sort;
      view.filter = filter;
      refresh(view);
      return { version: view.version, total: view.order.length, sort };
    },
    get_rows: ({ id, start, end }) => {
      const view = views.get(id)!;
      return { version: view.version, total: view.order.length, start, rows: view.order.slice(start, end) };
    },
    row_index: () => null,
    open_detail: ({ name, namespace, onEvent }) => {
      send(onEvent, {
        type: "snapshot",
        detail: {
          name,
          namespace,
          uid: "x",
          resourceVersion: "1",
          created: now - 3600,
          labels: { app: "web" },
          annotations: {},
          sections: [{ title: "Pod", fields: [{ label: "Status", value: "Running" }] }],
          conditions: [{ type: "Ready", status: "True", reason: null, message: null, lastTransition: now - 60 }],
          containers: [{ name: "app", init: false }],
          yaml: `apiVersion: v1\nkind: Pod\nmetadata:\n  name: ${name}\n  namespace: ${namespace}\n`,
        },
      });
      return nextSub++;
    },
    open_logs: ({ onEvent }) => {
      const lines = Array.from({ length: 5000 }, (_, i) => `2026-11-19T10:00:00Z line ${i}${i % 1000 === 0 ? " ERROR something failed" : ""}`);
      send(onEvent, { type: "lines", lines });
      return nextSub++;
    },
    close_subscription: ({ id }) => {
      views.delete(id);
      return null;
    },
    reveal_secret: () => ({}),
  };

  (window as any).__TAURI_INTERNALS__ = {
    transformCallback(callback: (msg: unknown) => void) {
      const id = nextCallback++;
      callbacks.set(id, callback);
      return id;
    },
    unregisterCallback(id: number) {
      callbacks.delete(id);
    },
    async invoke(cmd: string, args: Record<string, any> = {}) {
      const handler = handlers[cmd];
      if (!handler) throw { code: "internal", message: `mock: unknown command ${cmd}` };
      return handler(args);
    },
  };
  (window as any).__KV_MOCK__ = { views };
}
