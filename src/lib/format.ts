import { createRoot, createSignal } from "solid-js";
import type { AppError, AuthKind, Cell, ColumnKind } from "./types";

/** Age like kubectl: 42s, 7m, 3h12m, 5d, 2y45d. */
export function formatAge(createdSeconds: number, nowSeconds: number): string {
  const s = Math.max(0, Math.floor(nowSeconds - createdSeconds));
  if (s < 120) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 10) return `${m}m${s % 60 ? `${s % 60}s` : ""}`;
  if (m < 180) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 8) return `${h}h${m % 60 ? `${m % 60}m` : ""}`;
  if (h < 48) return `${h}h`;
  const d = Math.floor(h / 24);
  if (d < 8) return `${d}d${h % 24 ? `${h % 24}h` : ""}`;
  if (d < 365 * 2) return `${d}d`;
  const y = Math.floor(d / 365);
  if (y >= 8) return `${y}y`;
  const rest = d % 365;
  return `${y}y${rest ? `${rest}d` : ""}`;
}

export function formatBytes(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${unit === 0 ? value : value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

export function formatTimestamp(seconds: number): string {
  return new Date(seconds * 1000).toLocaleString();
}

/** Plain text of a cell for display (age needs `now`). */
export function cellText(cell: Cell, kind: ColumnKind, nowSeconds: number): string {
  if (cell === null) return "";
  if (typeof cell === "object") return cell.t;
  if (kind === "age" && typeof cell === "number") return formatAge(cell, nowSeconds);
  return String(cell);
}

export function authLabel(auth: AuthKind): string {
  switch (auth.type) {
    case "exec":
      return `exec: ${auth.command.split("/").pop() || auth.command}`;
    case "auth_provider":
      return `auth-provider: ${auth.name}`;
    case "token":
      return "token";
    case "client_certificate":
      return "client certificate";
    case "basic":
      return "basic auth";
    case "none":
      return "no credentials";
  }
}

/** Actionable hint for an error, shown under its message. */
export function errorHint(error: AppError): string | null {
  switch (error.code) {
    case "unauthorized":
      return "Credentials were rejected or expired. For exec/SSO contexts, log in again in a terminal (e.g. `aws sso login`, `gcloud auth login`) and reconnect.";
    case "forbidden":
      return "Your RBAC permissions do not allow this. Pick a namespace you have access to.";
    case "connect":
      return "The API server is unreachable. Check VPN/network and the server address in your kubeconfig.";
    case "kubeconfig":
      return "Fix the kubeconfig (KUBECONFIG or ~/.kube/config) and reload contexts.";
    default:
      return null;
  }
}

// One shared clock for every age cell: a single timer, fine-grained updates.
const clock = createRoot(() => {
  const [now, setNow] = createSignal(Date.now() / 1000);
  if (typeof window !== "undefined") {
    window.setInterval(() => setNow(Date.now() / 1000), 1000);
  }
  return now;
});

export const nowSeconds = clock;
