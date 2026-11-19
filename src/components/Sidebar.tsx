import { For, Show, createMemo, createSignal } from "solid-js";
import { authLabel } from "../lib/format";
import type { KindInfo } from "../lib/types";
import { session } from "../stores/session";
import { UpdateStatus } from "./UpdateStatus";

export function Sidebar() {
  const { state } = session;
  const current = createMemo(() => state.contexts?.contexts.find((c) => c.name === state.context));
  const groups = createMemo(() => {
    const out = new Map<string, KindInfo[]>();
    for (const k of state.kinds) {
      const list = out.get(k.group) ?? [];
      list.push(k);
      out.set(k.group, list);
    }
    return [...out.entries()];
  });

  return (
    <aside class="flex h-full w-60 shrink-0 flex-col border-r border-line bg-panel">
      <div class="space-y-3 border-b border-line p-3">
        <label class="block">
          <span class="mb-1 block text-[11px] font-medium uppercase tracking-wide text-muted">Context</span>
          <select
            aria-label="Context"
            class="w-full rounded border border-line bg-canvas px-2 py-1"
            onChange={(e) => void session.connect(e.currentTarget.value)}
            disabled={!state.contexts?.contexts.length}
          >
            <For each={state.contexts?.contexts ?? []}>
              {(c) => (
                <option value={c.name} selected={c.name === state.context}>
                  {c.name}
                </option>
              )}
            </For>
          </select>
        </label>
        <Show when={current()}>
          {(ctx) => (
            <div class="space-y-0.5 text-[11px] text-muted">
              <div class="truncate" title={ctx().server ?? ""}>
                {ctx().server}
              </div>
              <div>{authLabel(ctx().auth)}</div>
              <Show when={ctx().insecureSkipTlsVerify}>
                <div class="font-medium text-warn" title="insecure-skip-tls-verify is set in kubeconfig">
                  ⚠ TLS verification disabled
                </div>
              </Show>
              <Show when={state.cluster}>{(c) => <div>Kubernetes {c().serverVersion}</div>}</Show>
            </div>
          )}
        </Show>
        <Show when={state.cluster}>
          <NamespacePicker />
        </Show>
      </div>

      <nav class="flex-1 overflow-y-auto py-2" aria-label="Resource kinds">
        <For each={groups()}>
          {([group, kinds]) => (
            <div class="mb-2">
              <div class="px-3 py-1 text-[11px] font-medium uppercase tracking-wide text-muted">{group}</div>
              <For each={kinds}>
                {(k) => (
                  <button
                    class="block w-full truncate px-4 py-1 text-left hover:bg-hover"
                    classList={{ "bg-selected font-medium": state.kind === k.kind }}
                    aria-current={state.kind === k.kind ? "page" : undefined}
                    onClick={() => session.setKind(k.kind)}
                  >
                    {k.title}
                  </button>
                )}
              </For>
            </div>
          )}
        </For>
      </nav>

      <div class="space-y-2 border-t border-line p-2">
        <button class="w-full rounded px-2 py-1 text-left text-muted hover:bg-hover" onClick={() => void session.loadContexts()}>
          Reload kubeconfig
        </button>
        <div class="px-2">
          <UpdateStatus />
        </div>
      </div>
    </aside>
  );
}

const ALL = "__all__";

function NamespacePicker() {
  const { state } = session;
  const forbidden = () => state.cluster?.namespaces.forbidden ?? false;
  const [draft, setDraft] = createSignal("");

  return (
    <div>
      <label class="block">
        <span class="mb-1 block text-[11px] font-medium uppercase tracking-wide text-muted">Namespace</span>
        <select
          aria-label="Namespace"
          class="w-full rounded border border-line bg-canvas px-2 py-1"
          onFocus={() => void session.refreshNamespaces()}
          onChange={(e) => session.setNamespace(e.currentTarget.value === ALL ? null : e.currentTarget.value)}
        >
          <Show when={!forbidden()}>
            <option value={ALL} selected={state.namespace === null}>
              All namespaces
            </option>
          </Show>
          <For each={state.cluster?.namespaces.names ?? []}>
            {(n) => (
              <option value={n} selected={n === state.namespace}>
                {n}
              </option>
            )}
          </For>
          <Show when={state.namespace && !state.cluster?.namespaces.names.includes(state.namespace)}>
            <option value={state.namespace!} selected>
              {state.namespace}
            </option>
          </Show>
        </select>
      </label>
      <Show when={forbidden()}>
        <form
          class="mt-2"
          onSubmit={(e) => {
            e.preventDefault();
            const ns = draft().trim();
            if (ns) session.setNamespace(ns);
          }}
        >
          <div class="mb-1 text-[11px] text-muted">Listing namespaces is not allowed. Enter one you can access:</div>
          <input
            aria-label="Namespace name"
            class="w-full rounded border border-line bg-canvas px-2 py-1"
            placeholder="namespace"
            value={draft()}
            onInput={(e) => setDraft(e.currentTarget.value)}
          />
        </form>
      </Show>
    </div>
  );
}
