// Typed wrappers around Tauri commands. Every subscription returns a handle
// whose `close()` must run when the subscriber goes away (Solid `onCleanup`).

import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  AppError,
  ClusterInfo,
  ContextList,
  DetailEvent,
  DownloadEvent,
  DownloadRequest,
  FindResult,
  KindInfo,
  LogOpened,
  LogQuery,
  LogQueryResult,
  LogRequest,
  LogStatus,
  LogWindow,
  NamespaceList,
  QueryResult,
  ResourceKind,
  RowsWindow,
  SaveResult,
  SortSpec,
  TimeMode,
  ViewOpened,
  ViewUpdate,
} from "./types";

export function isAppError(value: unknown): value is AppError {
  return typeof value === "object" && value !== null && "code" in value && "message" in value;
}

/** Normalizes anything thrown by `invoke` into an AppError. */
export function toAppError(value: unknown): AppError {
  if (isAppError(value)) return value;
  return { code: "internal", message: value instanceof Error ? value.message : String(value) };
}

export interface Subscription {
  id: number;
  close(): void;
}

function subscription(id: number): Subscription {
  let closed = false;
  return {
    id,
    close() {
      if (closed) return;
      closed = true;
      void invoke("close_subscription", { id }).catch(() => {});
    },
  };
}

export const api = {
  resourceKinds: () => invoke<KindInfo[]>("resource_kinds"),
  listContexts: () => invoke<ContextList>("list_contexts"),
  connect: (context: string) => invoke<ClusterInfo>("connect", { context }),
  listNamespaces: (context: string) => invoke<NamespaceList>("list_namespaces", { context }),
  resetSubscriptions: () => invoke<void>("reset_subscriptions"),

  async openView(
    params: { context: string; kind: ResourceKind; namespace: string | null; sort: SortSpec | null; filter: string },
    onUpdate: (update: ViewUpdate) => void,
  ): Promise<ViewOpened & Subscription> {
    const channel = new Channel<ViewUpdate>(onUpdate);
    const opened = await invoke<ViewOpened>("open_view", { ...params, onUpdate: channel });
    return { ...opened, ...subscription(opened.id) };
  },

  setViewQuery: (id: number, sort: SortSpec, filter: string) =>
    invoke<QueryResult>("set_view_query", { id, sort, filter }),

  getRows: (id: number, start: number, end: number) => invoke<RowsWindow>("get_rows", { id, start, end }),

  rowIndex: (id: number, uid: string) => invoke<number | null>("row_index", { id, uid }),

  async openDetail(
    params: { context: string; kind: ResourceKind; namespace: string | null; name: string },
    onEvent: (event: DetailEvent) => void,
  ): Promise<Subscription> {
    const channel = new Channel<DetailEvent>(onEvent);
    const id = await invoke<number>("open_detail", { ...params, onEvent: channel });
    return subscription(id);
  },

  async openLogs(
    params: { context: string; namespace: string; pod: string; request: LogRequest },
    onStatus: (status: LogStatus) => void,
  ): Promise<LogOpened & Subscription> {
    const channel = new Channel<LogStatus>(onStatus);
    const opened = await invoke<LogOpened>("open_logs", { ...params, onStatus: channel });
    return { ...opened, ...subscription(opened.id) };
  },

  setLogQuery: (id: number, query: LogQuery | null) => invoke<LogQueryResult>("set_log_query", { id, query }),

  getLogLines: (id: number, start: number, end: number, timeMode: TimeMode) =>
    invoke<LogWindow>("get_log_lines", { id, start, end, timeMode }),

  findLogMatch: (id: number, from: number | null, forward: boolean) =>
    invoke<FindResult | null>("find_log_match", { id, from, forward }),

  copyLogLines: (id: number, start: number, end: number, timeMode: TimeMode) =>
    invoke<{ lines: number; bytes: number }>("copy_log_lines", { id, start, end, timeMode }),

  saveLoadedLogs: (id: number, path: string, timeMode: TimeMode, containerPrefix: boolean) =>
    invoke<SaveResult>("save_loaded_logs", { id, path, timeMode, containerPrefix }),

  async downloadLogs(
    params: { context: string; namespace: string; pod: string; request: DownloadRequest },
    onEvent: (event: DownloadEvent) => void,
  ): Promise<Subscription> {
    const channel = new Channel<DownloadEvent>(onEvent);
    const id = await invoke<number>("download_logs", { ...params, onEvent: channel });
    return subscription(id);
  },

  defaultLogPath: (fileName: string) => invoke<string>("default_log_path", { fileName }),

  revealSecret: (context: string, namespace: string, name: string) =>
    invoke<Record<string, string>>("reveal_secret", { context, namespace, name }),
};
