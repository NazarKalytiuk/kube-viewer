// Wire types mirroring the Rust command surface (src-tauri/src/commands.rs).

export type ErrorCode =
  | "kubeconfig"
  | "connect"
  | "unauthorized"
  | "forbidden"
  | "not_found"
  | "api"
  | "invalid_request"
  | "internal";

export interface AppError {
  code: ErrorCode;
  message: string;
}

export type AuthKind =
  | { type: "exec"; command: string }
  | { type: "auth_provider"; name: string }
  | { type: "token" }
  | { type: "client_certificate" }
  | { type: "basic" }
  | { type: "none" };

export interface ContextInfo {
  name: string;
  cluster: string;
  user: string | null;
  namespace: string | null;
  server: string | null;
  auth: AuthKind;
  insecureSkipTlsVerify: boolean;
}

export interface ContextList {
  contexts: ContextInfo[];
  current: string | null;
  sources: string[];
}

export interface NamespaceList {
  names: string[];
  forbidden: boolean;
}

export interface ClusterInfo {
  context: string;
  serverVersion: string;
  defaultNamespace: string | null;
  namespaces: NamespaceList;
}

export type ResourceKind =
  | "pods"
  | "deployments"
  | "replicaSets"
  | "statefulSets"
  | "daemonSets"
  | "services"
  | "ingresses"
  | "configMaps"
  | "secrets"
  | "persistentVolumeClaims"
  | "events"
  | "nodes"
  | "namespaces";

export interface KindInfo {
  kind: ResourceKind;
  title: string;
  group: string;
  namespaced: boolean;
}

export type ColumnKind = "text" | "number" | "age" | "status";

export interface Column {
  id: string;
  title: string;
  kind: ColumnKind;
  width: number;
}

export type Tone = "ok" | "warn" | "error" | "muted";

/** Text, number, unix seconds (age columns), status, or empty. */
export type Cell = string | number | { t: string; tone: Tone } | null;

export interface Row {
  uid: string;
  name: string;
  namespace: string | null;
  cells: Cell[];
}

export interface SortSpec {
  column: string;
  descending: boolean;
}

export interface ViewOpened {
  id: number;
  columns: Column[];
  sort: SortSpec;
}

export interface ViewUpdate {
  version: number;
  total: number;
  unfiltered: number;
  phase: "loading" | "ready";
  error: AppError | null;
}

export interface QueryResult {
  version: number;
  total: number;
  sort: SortSpec;
}

export interface RowsWindow {
  version: number;
  total: number;
  start: number;
  rows: Row[];
}

export interface Field {
  label: string;
  value: string;
}

export interface Section {
  title: string;
  fields: Field[];
}

export interface Condition {
  type: string;
  status: string;
  reason: string | null;
  message: string | null;
  lastTransition: number | null;
}

export interface ContainerRef {
  name: string;
  init: boolean;
}

export interface Detail {
  name: string;
  namespace: string | null;
  uid: string | null;
  resourceVersion: string | null;
  created: number | null;
  labels: Record<string, string>;
  annotations: Record<string, string>;
  sections: Section[];
  conditions: Condition[];
  containers: ContainerRef[];
  yaml: string;
}

export type DetailEvent =
  | { type: "snapshot"; detail: Detail }
  | { type: "deleted" }
  | { type: "error"; error: AppError };

export type TimeMode = "none" | "local" | "utc";

export interface LogRequest {
  containers: string[];
  /** Lines of history per container; null = all. */
  tailLines: number | null;
  /** Only history newer than this; null = no limit. */
  sinceSeconds: number | null;
  previous: boolean;
  follow: boolean;
}

export interface LogOpened {
  id: number;
  containers: string[];
}

export type LogPhase = "loading" | "following" | "reconnecting" | "ended";

export interface LogStatus {
  version: number;
  visible: number;
  lines: number;
  dropped: number;
  matches: number;
  phase: LogPhase;
  error: AppError | null;
}

export interface LogQuery {
  text: string;
  regex: boolean;
  caseSensitive: boolean;
  mode: "filter" | "find";
}

export interface LogQueryResult {
  version: number;
  visible: number;
  matches: number;
}

export interface LogLine {
  /** Line number in the session. */
  n: number;
  /** Formatted time (absent when hidden). */
  ts?: string;
  /** Container index. */
  c: number;
  text: string;
}

/** Lines `[start, end)`, or the newest `count` lines (see `get_log_lines`). */
export type LineWindow = { kind: "range"; start: number; end: number } | { kind: "tail"; count: number };

export interface LogWindow {
  version: number;
  total: number;
  start: number;
  lines: LogLine[];
}

export interface FindResult {
  index: number;
  ordinal: number;
  matches: number;
}

export interface DownloadRequest {
  containers: string[];
  previous: boolean;
  timeMode: TimeMode;
  containerPrefix: boolean;
  path: string;
}

export type DownloadEvent =
  | { type: "progress"; lines: number; bytes: number }
  | { type: "done"; path: string; lines: number; bytes: number }
  | { type: "error"; error: AppError };

export interface SaveResult {
  path: string;
  lines: number;
  bytes: number;
}
