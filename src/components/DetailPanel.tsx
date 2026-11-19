import { For, Match, Show, Suspense, Switch, createMemo, createSignal, lazy, onCleanup, onMount } from "solid-js";
import { api, toAppError, type Subscription } from "../lib/api";
import { formatAge, formatTimestamp, nowSeconds } from "../lib/format";
import type { AppError, Detail, ResourceKind } from "../lib/types";
import { ErrorNotice } from "./ErrorNotice";
import { LogsView } from "./LogsView";

// CodeMirror is most of the bundle; load it only when the YAML tab opens.
const YamlView = lazy(() => import("./YamlView").then((m) => ({ default: m.YamlView })));

type Tab = "status" | "labels" | "yaml" | "logs";

export interface DetailPanelProps {
  context: string;
  kind: ResourceKind;
  namespace: string | null;
  name: string;
  onClose: () => void;
  /** Panel fills the whole view (table hidden). */
  expanded: boolean;
  onToggleExpand: () => void;
}

let lastTab: Tab = "status";

export function DetailPanel(props: DetailPanelProps) {
  const [detail, setDetail] = createSignal<Detail | null>(null);
  const [deleted, setDeleted] = createSignal(false);
  const [error, setError] = createSignal<AppError | null>(null);
  const tabs = createMemo<Tab[]>(() => (props.kind === "pods" ? ["status", "labels", "yaml", "logs"] : ["status", "labels", "yaml"]));
  const [tab, setTab] = createSignal<Tab>(tabs().includes(lastTab) ? lastTab : "status");

  let sub: Subscription | null = null;
  let disposed = false;

  onMount(async () => {
    try {
      const s = await api.openDetail(
        { context: props.context, kind: props.kind, namespace: props.namespace, name: props.name },
        (event) => {
          if (event.type === "snapshot") {
            setDetail(event.detail);
            setDeleted(false);
            setError(null);
          } else if (event.type === "deleted") {
            setDeleted(true);
          } else {
            setError(event.error);
          }
        },
      );
      if (disposed) s.close();
      else sub = s;
    } catch (e) {
      if (!disposed) setError(toAppError(e));
    }
  });
  onCleanup(() => {
    disposed = true;
    sub?.close();
  });

  function choose(t: Tab) {
    lastTab = t;
    setTab(t);
  }

  return (
    <div class="flex h-full flex-col bg-canvas" onKeyDown={(e) => e.key === "Escape" && props.onClose()}>
      <header class="flex items-start gap-2 border-b border-line px-3 py-2">
        <div class="min-w-0 flex-1">
          <div class="selectable truncate text-[14px] font-semibold" title={props.name}>
            {props.name}
          </div>
          <div class="truncate text-[12px] text-muted">
            {props.namespace ? `${props.namespace} · ` : ""}
            <Show when={detail()?.created}>{(c) => <>created {formatAge(c(), nowSeconds())} ago</>}</Show>
          </div>
        </div>
        <button
          aria-label={props.expanded ? "Restore panel" : "Expand panel"}
          aria-pressed={props.expanded}
          title={props.expanded ? "Show the table again" : "Use the full width (great for logs)"}
          class="rounded px-2 text-[14px] text-muted hover:bg-hover hover:text-fg"
          onClick={() => props.onToggleExpand()}
        >
          {props.expanded ? "⇥" : "⇤"}
        </button>
        <button aria-label="Close details" class="rounded px-2 text-[16px] text-muted hover:bg-hover hover:text-fg" onClick={() => props.onClose()}>
          ×
        </button>
      </header>

      <div role="tablist" class="flex gap-1 border-b border-line px-2">
        <For each={tabs()}>
          {(t) => (
            <button
              role="tab"
              aria-selected={tab() === t}
              class="border-b-2 px-3 py-1.5 capitalize"
              classList={{ "border-accent text-fg": tab() === t, "border-transparent text-muted hover:text-fg": tab() !== t }}
              onClick={() => choose(t)}
            >
              {t === "yaml" ? "YAML" : t}
            </button>
          )}
        </For>
      </div>

      <Show when={deleted()}>
        <div class="border-b border-line bg-error-bg px-3 py-1.5 text-error">This object was deleted.</div>
      </Show>
      <Show when={error()}>
        {(e) => (
          <div class="p-3">
            <ErrorNotice compact error={e()} title="Cannot load details" />
          </div>
        )}
      </Show>

      <div class="min-h-0 flex-1">
        <Show when={detail()} fallback={<Show when={!error()}><div class="p-4 text-muted">Loading…</div></Show>}>
          {(d) => (
            <Switch>
              <Match when={tab() === "status"}>
                <StatusTab detail={d()} kind={props.kind} context={props.context} />
              </Match>
              <Match when={tab() === "labels"}>
                <LabelsTab detail={d()} />
              </Match>
              <Match when={tab() === "yaml"}>
                <Suspense fallback={<div class="p-4 text-muted">Loading editor…</div>}>
                  <YamlView text={d().yaml} />
                </Suspense>
              </Match>
              <Match when={tab() === "logs" && props.namespace}>
                <LogsView context={props.context} namespace={props.namespace!} pod={props.name} containers={d().containers} />
              </Match>
            </Switch>
          )}
        </Show>
      </div>
    </div>
  );
}

function StatusTab(props: { detail: Detail; kind: ResourceKind; context: string }) {
  return (
    <div class="selectable h-full space-y-4 overflow-auto p-3">
      <For each={props.detail.sections.filter((s) => s.fields.length > 0)}>
        {(section) => (
          <section>
            <h2 class="mb-1 text-[11px] font-semibold uppercase tracking-wide text-muted">{section.title}</h2>
            <dl class="grid grid-cols-[minmax(110px,max-content)_1fr] gap-x-4 gap-y-1">
              <For each={section.fields}>
                {(f) => (
                  <>
                    <dt class="text-muted">{f.label}</dt>
                    <dd class="whitespace-pre-wrap break-words font-mono text-[12px]">{f.value}</dd>
                  </>
                )}
              </For>
            </dl>
          </section>
        )}
      </For>

      <Show when={props.kind === "secrets" && props.detail.namespace}>
        <SecretValues context={props.context} namespace={props.detail.namespace!} name={props.detail.name} />
      </Show>

      <Show when={props.detail.conditions.length}>
        <section>
          <h2 class="mb-1 text-[11px] font-semibold uppercase tracking-wide text-muted">Conditions</h2>
          <table class="w-full border-collapse text-left">
            <thead class="text-muted">
              <tr>
                <th class="py-1 pr-3 font-medium">Type</th>
                <th class="py-1 pr-3 font-medium">Status</th>
                <th class="py-1 pr-3 font-medium">Reason</th>
                <th class="py-1 font-medium">Since</th>
              </tr>
            </thead>
            <tbody>
              <For each={props.detail.conditions}>
                {(c) => (
                  <tr class="border-t border-line align-top" title={c.message ?? ""}>
                    <td class="py-1 pr-3">{c.type}</td>
                    <td class="py-1 pr-3" classList={{ "text-ok": c.status === "True", "text-warn": c.status !== "True" }}>
                      {c.status}
                    </td>
                    <td class="py-1 pr-3">
                      {c.reason}
                      <Show when={c.message}>
                        <div class="text-[12px] text-muted">{c.message}</div>
                      </Show>
                    </td>
                    <td class="py-1 tabular-nums text-muted" title={c.lastTransition ? formatTimestamp(c.lastTransition) : ""}>
                      {c.lastTransition ? formatAge(c.lastTransition, nowSeconds()) : ""}
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </section>
      </Show>
    </div>
  );
}

function SecretValues(props: { context: string; namespace: string; name: string }) {
  const [values, setValues] = createSignal<Record<string, string> | null>(null);
  const [error, setError] = createSignal<AppError | null>(null);
  async function reveal() {
    try {
      setValues(await api.revealSecret(props.context, props.namespace, props.name));
      setError(null);
    } catch (e) {
      setError(toAppError(e));
    }
  }
  return (
    <section>
      <div class="mb-1 flex items-center gap-2">
        <h2 class="text-[11px] font-semibold uppercase tracking-wide text-muted">Values</h2>
        <Show
          when={values()}
          fallback={
            <button class="rounded border border-line px-2 py-0.5 hover:bg-hover" onClick={() => void reveal()}>
              Reveal values
            </button>
          }
        >
          <button class="rounded border border-line px-2 py-0.5 hover:bg-hover" onClick={() => setValues(null)}>
            Hide
          </button>
        </Show>
      </div>
      <Show when={error()}>{(e) => <ErrorNotice compact error={e()} title="Cannot read secret" />}</Show>
      <Show when={values()}>
        {(v) => (
          <dl class="grid grid-cols-[minmax(110px,max-content)_1fr] gap-x-4 gap-y-1">
            <For each={Object.entries(v())}>
              {([key, value]) => (
                <>
                  <dt class="text-muted">{key}</dt>
                  <dd class="whitespace-pre-wrap break-all font-mono text-[12px]">{value}</dd>
                </>
              )}
            </For>
          </dl>
        )}
      </Show>
    </section>
  );
}

function LabelsTab(props: { detail: Detail }) {
  return (
    <div class="selectable h-full space-y-4 overflow-auto p-3">
      <KeyValues title="Labels" map={props.detail.labels} />
      <KeyValues title="Annotations" map={props.detail.annotations} />
    </div>
  );
}

function KeyValues(props: { title: string; map: Record<string, string> }) {
  const entries = () => Object.entries(props.map);
  return (
    <section>
      <h2 class="mb-1 text-[11px] font-semibold uppercase tracking-wide text-muted">
        {props.title} <span class="font-normal">({entries().length})</span>
      </h2>
      <Show when={entries().length} fallback={<div class="text-muted">None</div>}>
        <dl class="grid grid-cols-[minmax(140px,max-content)_1fr] gap-x-4 gap-y-1">
          <For each={entries()}>
            {([k, v]) => (
              <>
                <dt class="break-all font-mono text-[12px] text-muted">{k}</dt>
                <dd class="max-h-40 overflow-auto whitespace-pre-wrap break-all font-mono text-[12px]">{v}</dd>
              </>
            )}
          </For>
        </dl>
      </Show>
    </section>
  );
}
