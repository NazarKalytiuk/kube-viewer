// In-page fake of the Tauri backend, injected with `page.addInitScript`.
// It implements `window.__TAURI_INTERNALS__` (invoke + channel callbacks)
// so the real frontend runs unmodified in a browser.

export interface MockOptions {
  podCount: number;
  /** Log lines per container in the pod log mock. */
  logLines?: number;
  /** Release announced by the update server; none means up to date. */
  update?: { version: string; notes?: string; fail?: string };
}

// This function is serialized into the page: it must be self-contained.
export function installMockBackend(options: MockOptions) {
  type Row = { uid: string; name: string; namespace: string | null; cells: unknown[] };
  type View = { rows: Row[]; order: Row[]; sort: { column: string; descending: boolean }; filter: string; version: number; columns: { id: string }[] };

  const callbacks = new Map<number, (msg: unknown) => void>();
  let nextCallback = 1;
  const channelIndex = new Map<number, number>();
  const views = new Map<number, View>();
  type MockLine = { ms: number; c: number; text: string };
  type LogSession = { lines: MockLine[]; query: { re: RegExp; mode: string } | null; channel: { id: number }; version: number; containers: string[] };
  const logSessions = new Map<number, LogSession>();
  const calls: { cmd: string; args: Record<string, any> }[] = [];
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

  function matches(s: LogSession) {
    return s.query ? s.lines.filter((l) => s.query!.re.test(l.text)) : [];
  }
  function visibleLines(s: LogSession) {
    const all = s.lines.map((line, i) => ({ line, n: i + 1 }));
    return s.query?.mode === "filter" ? all.filter((x) => s.query!.re.test(x.line.text)) : all;
  }
  // Like the backend: a closed session's id is unknown.
  function logSession(id: number): LogSession {
    const s = logSessions.get(id);
    if (!s) throw { code: "invalid_request", message: `unknown log view ${id}` };
    return s;
  }

  function sendLogStatus(s: LogSession) {
    send(s.channel, {
      version: s.version,
      visible: visibleLines(s).length,
      lines: s.lines.length,
      dropped: 0,
      matches: matches(s).length,
      phase: "following",
      error: null,
    });
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
          containers: [
            { name: "app", init: false },
            { name: "sidecar", init: false },
          ],
          yaml: `apiVersion: v1\nkind: Pod\nmetadata:\n  name: ${name}\n  namespace: ${namespace}\n`,
        },
      });
      return nextSub++;
    },
    open_logs: ({ request, onStatus }) => {
      const containers: string[] = request.containers;
      const count = options.logLines ?? 50_000;
      // Like `tailLines`: the newest lines of each container.
      const first = request.tailLines == null ? 0 : Math.max(0, count - request.tailLines);
      const base = Date.UTC(2026, 10, 19, 10, 0, 0);
      const lines: MockLine[] = [];
      for (let i = first; i < count; i++) {
        containers.forEach((_, c) => {
          const level =
            i % 1000 === 0
              ? "ERROR something failed"
              : i % 500 === 0
                ? "WARN slow request"
                : i % 7 === 0
                  ? "\x1b[32mGET\x1b[0m /api/orders \x1b[36m200\x1b[0m 12ms"
                  : "INFO ok";
          lines.push({ ms: base + i * 10 + c, c, text: `${containers[c]} line ${i} ${level}` });
        });
      }
      const id = nextSub++;
      // Like `LogStore`: every stored line bumps the version.
      const session: LogSession = { lines, query: null, channel: onStatus, version: lines.length, containers };
      logSessions.set(id, session);
      sendLogStatus(session);
      return { id, containers };
    },
    set_log_query: ({ id, query }) => {
      const s = logSession(id);
      if (query) {
        let re: RegExp;
        try {
          re = new RegExp(query.regex ? query.text : query.text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), query.caseSensitive ? "" : "i");
        } catch (e) {
          throw { code: "invalid_request", message: `invalid regular expression: ${(e as Error).message}` };
        }
        s.query = { re, mode: query.mode };
      } else {
        s.query = null;
      }
      s.version++;
      const v = visibleLines(s);
      return { version: s.version, visible: v.length, matches: matches(s).length };
    },
    get_log_lines: ({ id, start, end, timeMode }) => {
      const s = logSession(id);
      const v = visibleLines(s);
      const lines = v.slice(start, end).map(({ line, n }) => ({
        n,
        c: line.c,
        text: line.text,
        ...(timeMode === "utc" ? { ts: new Date(line.ms).toISOString() } : timeMode === "local" ? { ts: `L ${new Date(line.ms).toISOString().slice(0, 23).replace("T", " ")}` } : {}),
      }));
      return { version: s.version, total: v.length, start, lines };
    },
    find_log_match: ({ id, from, forward }) => {
      const s = logSession(id);
      const v = visibleLines(s);
      const idx = v.map((x, i) => (s.query && s.query.re.test(x.line.text) ? i : -1)).filter((i) => i >= 0);
      if (!idx.length) return null;
      let k: number;
      if (from === null) k = forward ? 0 : idx.length - 1;
      else if (forward) {
        k = idx.findIndex((i) => i > from);
        if (k < 0) k = 0;
      } else {
        k = -1;
        for (let j = idx.length - 1; j >= 0; j--) if (idx[j]! < from) { k = j; break; }
        if (k < 0) k = idx.length - 1;
      }
      return { index: idx[k], ordinal: k + 1, matches: idx.length };
    },
    default_log_path: ({ fileName }) => `/Users/test/Downloads/${fileName}`,
    "plugin:dialog|save": ({ options }) => options.defaultPath,
    "plugin:opener|reveal_item_in_dir": () => null,
    "plugin:app|version": () => "0.1.0",
    "plugin:updater|check": () =>
      options.update
        ? { rid: 1, currentVersion: "0.1.0", version: options.update.version, body: options.update.notes, rawJson: {} }
        : null,
    "plugin:updater|download_and_install": ({ onEvent }) => {
      if (options.update?.fail) throw options.update.fail;
      const total = 12 * 1024 * 1024;
      send(onEvent, { event: "Started", data: { contentLength: total } });
      for (let i = 0; i < 4; i++) send(onEvent, { event: "Progress", data: { chunkLength: total / 4 } });
      send(onEvent, { event: "Finished" });
      return null;
    },
    "plugin:process|restart": () => null,
    "plugin:resources|close": () => null,
    download_logs: ({ request, onEvent }) => {
      const id = nextSub++;
      const total = (options.logLines ?? 50_000) * request.containers.length;
      send(onEvent, { type: "progress", lines: Math.floor(total / 2), bytes: total * 20 });
      send(onEvent, { type: "done", path: request.path, lines: total, bytes: total * 40 });
      return id;
    },
    copy_log_lines: ({ id, start, end }) => {
      const s = logSession(id);
      const v = visibleLines(s).slice(Math.min(start, end), Math.max(start, end) + 1);
      return { lines: v.length, bytes: v.reduce((n, x) => n + x.line.text.length + 1, 0) };
    },
    save_loaded_logs: ({ id, path }) => {
      const s = logSession(id);
      const n = visibleLines(s).length;
      return { path, lines: n, bytes: n * 40 };
    },
    close_subscription: ({ id }) => {
      views.delete(id);
      logSessions.delete(id);
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
      calls.push({ cmd, args });
      const handler = handlers[cmd];
      if (!handler) throw { code: "internal", message: `mock: unknown command ${cmd}` };
      return handler(args);
    },
  };
  (window as any).__KV_MOCK__ = {
    views,
    calls,
    /** Appends live lines to every open log session. */
    appendLogs(count: number) {
      for (const s of logSessions.values()) {
        const last = s.lines[s.lines.length - 1]?.ms ?? Date.now();
        for (let i = 0; i < count; i++) s.lines.push({ ms: last + (i + 1) * 10, c: 0, text: `${s.containers[0]} live ${i}` });
        s.version += count;
        sendLogStatus(s);
      }
    },
  };
}
