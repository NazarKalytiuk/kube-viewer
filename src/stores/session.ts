// App-wide session state: contexts, connection, namespace, kind, selection.

import { createRoot } from "solid-js";
import { createStore } from "solid-js/store";
import { api, toAppError } from "../lib/api";
import type { AppError, ClusterInfo, ContextList, KindInfo, ResourceKind } from "../lib/types";

export interface Selection {
  uid: string;
  name: string;
  namespace: string | null;
}

interface SessionState {
  kinds: KindInfo[];
  contexts: ContextList | null;
  contextsError: AppError | null;
  context: string | null;
  cluster: ClusterInfo | null;
  connecting: boolean;
  connectError: AppError | null;
  /** `null` = all namespaces. */
  namespace: string | null;
  kind: ResourceKind;
  selection: Selection | null;
}

const storage = {
  get(key: string): string | null {
    try {
      return window.localStorage.getItem(`kubeviewer:${key}`);
    } catch {
      return null;
    }
  },
  set(key: string, value: string | null) {
    try {
      if (value === null) window.localStorage.removeItem(`kubeviewer:${key}`);
      else window.localStorage.setItem(`kubeviewer:${key}`, value);
    } catch {
      // Storage unavailable: preferences just aren't remembered.
    }
  },
};

const ALL_NAMESPACES = "\u0000all";

function createSession() {
  const [state, setState] = createStore<SessionState>({
    kinds: [],
    contexts: null,
    contextsError: null,
    context: null,
    cluster: null,
    connecting: false,
    connectError: null,
    namespace: null,
    kind: (storage.get("kind") as ResourceKind | null) ?? "pods",
    selection: null,
  });

  // Each connect attempt gets a token; late answers for an older attempt are ignored.
  let connectToken = 0;

  async function init() {
    await api.resetSubscriptions().catch(() => {});
    const kinds = await api.resourceKinds();
    setState("kinds", kinds);
    if (!kinds.some((k) => k.kind === state.kind)) setState("kind", "pods");
    await loadContexts();
  }

  async function loadContexts() {
    try {
      const contexts = await api.listContexts();
      setState({ contexts, contextsError: null });
      const remembered = storage.get("context");
      const names = contexts.contexts.map((c) => c.name);
      const initial =
        (remembered && names.includes(remembered) ? remembered : null) ??
        (contexts.current && names.includes(contexts.current) ? contexts.current : null) ??
        names[0] ??
        null;
      if (initial && initial !== state.context) await connect(initial);
    } catch (error) {
      setState({ contexts: null, contextsError: toAppError(error) });
    }
  }

  async function connect(context: string) {
    const token = ++connectToken;
    setState({ context, cluster: null, connecting: true, connectError: null, selection: null });
    storage.set("context", context);
    try {
      const cluster = await api.connect(context);
      if (token !== connectToken) return;
      setState({ cluster, connecting: false, namespace: initialNamespace(context, cluster) });
    } catch (error) {
      if (token !== connectToken) return;
      setState({ connecting: false, connectError: toAppError(error) });
    }
  }

  function initialNamespace(context: string, cluster: ClusterInfo): string | null {
    const remembered = storage.get(`namespace:${context}`);
    if (cluster.namespaces.forbidden) {
      // Cannot list namespaces: stay inside the one we know is allowed.
      return remembered && remembered !== ALL_NAMESPACES ? remembered : (cluster.namespaces.names[0] ?? "default");
    }
    if (remembered === ALL_NAMESPACES) return null;
    if (remembered && cluster.namespaces.names.includes(remembered)) return remembered;
    return cluster.defaultNamespace && cluster.namespaces.names.includes(cluster.defaultNamespace)
      ? cluster.defaultNamespace
      : null;
  }

  function setNamespace(namespace: string | null) {
    setState({ namespace, selection: null });
    if (state.context) storage.set(`namespace:${state.context}`, namespace ?? ALL_NAMESPACES);
  }

  function setKind(kind: ResourceKind) {
    setState({ kind, selection: null });
    storage.set("kind", kind);
  }

  function select(selection: Selection | null) {
    // Re-selecting the same object must not restart its detail subscription.
    if (selection && state.selection?.uid === selection.uid) return;
    setState("selection", selection);
  }

  async function refreshNamespaces() {
    const context = state.context;
    if (!context || !state.cluster) return;
    try {
      const namespaces = await api.listNamespaces(context);
      if (state.context === context) setState("cluster", "namespaces", namespaces);
    } catch {
      // Keep the previous list; the view itself reports access errors.
    }
  }

  const kindInfo = () => state.kinds.find((k) => k.kind === state.kind);

  return { state, init, loadContexts, connect, setNamespace, setKind, select, refreshNamespaces, kindInfo };
}

export const session = createRoot(createSession);
