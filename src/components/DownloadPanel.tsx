// Export logs to a file: the full log from the cluster (streamed to disk) or
// exactly the lines loaded in the viewer (honouring the filter).

import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { save } from "@tauri-apps/plugin-dialog";
import { Match, Show, Switch, createSignal, onCleanup } from "solid-js";
import { api, toAppError, type Subscription } from "../lib/api";
import { formatBytes } from "../lib/format";
import type { AppError, TimeMode } from "../lib/types";
import { Segmented } from "./LogsView";

type Scope = "full" | "loaded";

type Job =
  | { state: "running"; lines: number; bytes: number }
  | { state: "done"; path: string; lines: number; bytes: number }
  | { state: "error"; error: AppError };

export interface DownloadPanelProps {
  context: string;
  namespace: string;
  pod: string;
  containers: string[];
  previous: boolean;
  timeMode: TimeMode;
  /** The viewer shows filtered lines. */
  filtered: boolean;
  loadedLines: number;
  sessionId: number | null;
  onClose: () => void;
}

export function suggestedFileName(pod: string, containers: string[], previous: boolean, now = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  const stamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
  const which = containers.length === 1 ? containers[0] : "all-containers";
  return `${pod}_${which}${previous ? "_previous" : ""}_${stamp}.log`;
}

export function DownloadPanel(props: DownloadPanelProps) {
  const [scope, setScope] = createSignal<Scope>("full");
  // Follows the viewer's time mode until the user picks one here. A file
  // without times is rarely wanted, so "No time" in the viewer maps to UTC.
  const [pickedTime, setPickedTime] = createSignal<TimeMode | null>(null);
  const timeMode = () => pickedTime() ?? (props.timeMode === "none" ? "utc" : props.timeMode);
  const setTimeMode = (mode: TimeMode) => setPickedTime(mode);
  const [prefix, setPrefix] = createSignal(props.containers.length > 1);
  const [job, setJob] = createSignal<Job | null>(null);
  let sub: Subscription | null = null;
  onCleanup(() => sub?.close());

  async function start() {
    const fileName = suggestedFileName(props.pod, props.containers, props.previous);
    let defaultPath: string | undefined;
    try {
      defaultPath = await api.defaultLogPath(fileName);
    } catch {
      defaultPath = fileName;
    }
    const path = await save({ title: "Save logs", defaultPath, filters: [{ name: "Log files", extensions: ["log", "txt"] }] });
    if (!path) return;

    if (scope() === "loaded") {
      if (props.sessionId === null) return;
      setJob({ state: "running", lines: 0, bytes: 0 });
      try {
        const r = await api.saveLoadedLogs(props.sessionId, path, timeMode(), prefix());
        setJob({ state: "done", path: r.path, lines: r.lines, bytes: r.bytes });
      } catch (e) {
        setJob({ state: "error", error: toAppError(e) });
      }
      return;
    }

    setJob({ state: "running", lines: 0, bytes: 0 });
    try {
      sub = await api.downloadLogs(
        {
          context: props.context,
          namespace: props.namespace,
          pod: props.pod,
          request: { containers: props.containers, previous: props.previous, timeMode: timeMode(), containerPrefix: prefix(), path },
        },
        (event) => {
          if (event.type === "progress") setJob({ state: "running", lines: event.lines, bytes: event.bytes });
          else if (event.type === "done") setJob({ state: "done", path: event.path, lines: event.lines, bytes: event.bytes });
          else setJob({ state: "error", error: event.error });
        },
      );
    } catch (e) {
      setJob({ state: "error", error: toAppError(e) });
    }
  }

  function cancel() {
    sub?.close();
    sub = null;
    setJob(null);
  }

  const running = () => job()?.state === "running";

  return (
    <section aria-label="Download logs" class="space-y-2 border-b border-line bg-panel px-3 py-2 text-[12px]">
      <div class="flex flex-wrap items-center gap-x-4 gap-y-2">
        <Segmented
          label="What to save"
          options={[
            { value: "full", label: "Full log from cluster", title: "Everything the kubelet has for the selected container(s), streamed to disk" },
            {
              value: "loaded",
              label: `Loaded lines (${props.loadedLines.toLocaleString()}${props.filtered ? ", filtered" : ""})`,
              title: "Exactly the lines currently loaded in the viewer",
            },
          ]}
          value={scope()}
          onChange={setScope}
        />
        <label class="flex items-center gap-1">
          Time
          <select aria-label="Time format" class="rounded border border-line bg-canvas px-1 py-0.5" onChange={(e) => setTimeMode(e.currentTarget.value as TimeMode)}>
            <option value="utc" selected={timeMode() === "utc"}>
              UTC
            </option>
            <option value="local" selected={timeMode() === "local"}>
              Local
            </option>
            <option value="none" selected={timeMode() === "none"}>
              None
            </option>
          </select>
        </label>
        <label class="flex items-center gap-1">
          <input type="checkbox" checked={prefix()} onChange={(e) => setPrefix(e.currentTarget.checked)} />
          Container name per line
        </label>
        <div class="flex-1" />
        <button class="rounded bg-accent px-3 py-1 font-medium text-accent-fg disabled:opacity-50" disabled={running()} onClick={() => void start()}>
          Save as…
        </button>
        <button aria-label="Close download panel" class="rounded px-2 text-muted hover:bg-hover hover:text-fg" onClick={() => props.onClose()}>
          ×
        </button>
      </div>
      <div class="text-muted">
        {props.containers.length > 1 ? `Containers: ${props.containers.join(", ")} (merged by time)` : `Container: ${props.containers[0] ?? ""}`}
        {props.previous ? " · previous instance" : ""}
      </div>
      <Show when={job()}>
        {(j) => (
          <Switch>
            <Match when={j().state === "running" && (j() as Extract<Job, { state: "running" }>)}>
              {(r) => (
                <div class="flex items-center gap-3" aria-live="polite">
                  <span>
                    Saving… {r().lines.toLocaleString()} lines · {formatBytes(r().bytes)}
                  </span>
                  <Show when={scope() === "full"}>
                    <button class="rounded border border-line px-2 py-0.5 hover:bg-hover" onClick={cancel}>
                      Cancel
                    </button>
                  </Show>
                </div>
              )}
            </Match>
            <Match when={j().state === "done" && (j() as Extract<Job, { state: "done" }>)}>
              {(d) => (
                <div class="flex items-center gap-3" aria-live="polite">
                  <span class="text-ok">
                    Saved {d().lines.toLocaleString()} lines ({formatBytes(d().bytes)}) to {d().path.split("/").pop()}
                  </span>
                  <button class="rounded border border-line px-2 py-0.5 hover:bg-hover" onClick={() => void revealItemInDir(d().path)}>
                    Show in Finder
                  </button>
                </div>
              )}
            </Match>
            <Match when={j().state === "error" && (j() as Extract<Job, { state: "error" }>)}>
              {(e) => <div class="font-mono text-error">{e().error.message}</div>}
            </Match>
          </Switch>
        )}
      </Show>
    </section>
  );
}
