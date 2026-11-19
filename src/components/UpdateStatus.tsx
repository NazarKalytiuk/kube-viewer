// Sidebar footer: app version, update check, download progress and restart.

import { Match, Show, Switch } from "solid-js";
import { formatBytes } from "../lib/format";
import { updater, type UpdateState } from "../stores/updater";

type Of<K extends UpdateState["kind"]> = Extract<UpdateState, { kind: K }>;

export function UpdateStatus() {
  const s = updater.state;
  const is = <K extends UpdateState["kind"]>(kind: K) => (s().kind === kind ? (s() as Of<K>) : false);
  const button = "rounded border border-line bg-canvas px-2 py-0.5 hover:bg-hover disabled:opacity-50";

  return (
    <section aria-label="Updates" class="space-y-1.5 text-[11px]">
      <div class="flex items-center gap-2">
        <span class="flex-1 truncate text-muted">KubeViewer {updater.currentVersion() ?? ""}</span>
        <Show when={s().kind === "idle" || s().kind === "upToDate" || s().kind === "error" || s().kind === "checking"}>
          <button class={button} disabled={updater.busy()} onClick={() => void updater.check()}>
            {s().kind === "checking" ? "Checking…" : "Check for updates"}
          </button>
        </Show>
      </div>
      <Switch>
        <Match when={is("upToDate")}>
          <div class="text-muted" aria-live="polite">
            You have the latest version.
          </div>
        </Match>
        <Match when={is("error")}>
          {(e) => (
            <div role="alert" class="selectable break-words text-error">
              Update failed: {e().message}
            </div>
          )}
        </Match>
        <Match when={is("available")}>
          {(a) => (
            <div class="space-y-1 rounded border border-accent/40 bg-selected p-2" aria-live="polite">
              <div class="flex items-center gap-2">
                <span class="flex-1 font-medium">Version {a().version} is available</span>
                <button class="rounded bg-accent px-2 py-0.5 font-medium text-accent-fg" onClick={() => void updater.install()}>
                  Update
                </button>
              </div>
              <Show when={a().notes}>
                {(notes) => (
                  <details>
                    <summary class="cursor-default text-muted">What’s new</summary>
                    <div class="selectable mt-1 max-h-40 overflow-y-auto whitespace-pre-wrap">{notes()}</div>
                  </details>
                )}
              </Show>
            </div>
          )}
        </Match>
        <Match when={is("downloading")}>
          {(d) => (
            <div class="space-y-1" aria-live="polite">
              <div>
                Downloading {d().version}… {formatBytes(d().received)}
                {d().total ? ` of ${formatBytes(d().total!)}` : ""}
              </div>
              <Show when={d().total}>
                {(total) => (
                  <div
                    role="progressbar"
                    aria-label="Update download"
                    aria-valuemin={0}
                    aria-valuemax={total()}
                    aria-valuenow={d().received}
                    class="h-1 overflow-hidden rounded bg-hover"
                  >
                    <div class="h-full bg-accent" style={{ width: `${Math.min(100, (d().received / total()) * 100)}%` }} />
                  </div>
                )}
              </Show>
            </div>
          )}
        </Match>
        <Match when={is("ready")}>
          {(r) => (
            <div class="flex items-center gap-2 rounded border border-ok/40 p-2" aria-live="polite">
              <span class="flex-1">Version {r().version} is installed. Restart to use it.</span>
              <button class="rounded bg-accent px-2 py-0.5 font-medium text-accent-fg" onClick={() => void updater.restart()}>
                Restart
              </button>
            </div>
          )}
        </Match>
      </Switch>
    </section>
  );
}
