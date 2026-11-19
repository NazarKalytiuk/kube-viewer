// In-app updates from GitHub Releases.
//
// The updater plugin fetches `latest.json` from the endpoint in tauri.conf.json
// and verifies the downloaded bundle against the embedded minisign public key.
// Installing replaces the .app on disk; the running app keeps working until the
// user restarts, so open log streams are never cut off without consent.

import { getVersion } from "@tauri-apps/api/app";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { createSignal } from "solid-js";
import { toAppError } from "../lib/api";

/** Automatic checks: shortly after launch, then periodically. */
const FIRST_CHECK_DELAY_MS = 5_000;
const CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;
const CHECK_TIMEOUT_MS = 30_000;

export type UpdateState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "upToDate" }
  | { kind: "available"; version: string; notes: string | null }
  | { kind: "downloading"; version: string; received: number; total: number | null }
  | { kind: "ready"; version: string }
  | { kind: "error"; message: string };

const [state, setState] = createSignal<UpdateState>({ kind: "idle" });
const [currentVersion, setCurrentVersion] = createSignal<string | null>(null);
let pending: Update | null = null;
let timer: ReturnType<typeof setInterval> | undefined;

const busy = () => {
  const k = state().kind;
  return k === "checking" || k === "downloading" || k === "ready";
};

/**
 * Asks the update server for a newer release. Automatic checks stay silent on
 * failure (offline laptops are normal); manual checks report the error.
 */
async function checkNow(manual: boolean): Promise<void> {
  if (busy()) return;
  const before = state();
  setState({ kind: "checking" });
  try {
    const update = await check({ timeout: CHECK_TIMEOUT_MS });
    await pending?.close().catch(() => {});
    pending = update;
    setState(update ? { kind: "available", version: update.version, notes: update.body?.trim() || null } : { kind: "upToDate" });
  } catch (e) {
    setState(manual ? { kind: "error", message: toAppError(e).message } : before.kind === "available" ? before : { kind: "idle" });
  }
}

async function install(): Promise<void> {
  const update = pending;
  if (!update || state().kind !== "available") return;
  let received = 0;
  let total: number | null = null;
  // Channel messages and the command's reply travel separately, so progress
  // events may still arrive after downloadAndInstall has resolved.
  let finished = false;
  setState({ kind: "downloading", version: update.version, received, total });
  try {
    await update.downloadAndInstall((event) => {
      if (finished) return;
      if (event.event === "Started") total = event.data.contentLength ?? null;
      else if (event.event === "Progress") received += event.data.chunkLength;
      setState({ kind: "downloading", version: update.version, received, total });
    });
    finished = true;
    setState({ kind: "ready", version: update.version });
  } catch (e) {
    finished = true;
    setState({ kind: "error", message: toAppError(e).message });
  }
}

/** Starts version lookup and, in production builds, periodic checks. */
function init(): void {
  void getVersion()
    .then(setCurrentVersion)
    .catch(() => setCurrentVersion(null));
  // Dev builds are not bundled apps, so installing over them is meaningless.
  if (!import.meta.env.PROD || timer !== undefined) return;
  setTimeout(() => void checkNow(false), FIRST_CHECK_DELAY_MS);
  timer = setInterval(() => void checkNow(false), CHECK_INTERVAL_MS);
}

export const updater = {
  state,
  currentVersion,
  busy,
  init,
  check: () => checkNow(true),
  install,
  restart: () => relaunch(),
};
