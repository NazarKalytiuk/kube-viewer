import { Match, Show, Switch, onMount } from "solid-js";
import { ErrorNotice } from "./components/ErrorNotice";
import { ResourceView } from "./components/ResourceView";
import { Sidebar } from "./components/Sidebar";
import { session } from "./stores/session";
import { updater } from "./stores/updater";

export default function App() {
  const { state } = session;
  onMount(() => {
    void session.init();
    updater.init();
  });

  // Remount the view whenever context, kind or namespace changes so each
  // combination owns exactly one backend subscription.
  const effectiveNamespace = () => (session.kindInfo()?.namespaced ? state.namespace : null);
  const viewKey = () =>
    state.cluster && state.context ? `${state.context}\u0000${state.kind}\u0000${effectiveNamespace() ?? ""}` : null;

  return (
    <div class="flex h-full">
      <Sidebar />
      <main class="flex min-w-0 flex-1 flex-col">
        <Switch>
          <Match when={state.contextsError}>
            {(err) => (
              <Centered>
                <ErrorNotice error={err()} title="Cannot read kubeconfig" onRetry={() => void session.loadContexts()} />
              </Centered>
            )}
          </Match>
          <Match when={state.contexts && state.contexts.contexts.length === 0}>
            <Centered>
              <div class="text-muted">No contexts found in {state.contexts?.sources.join(", ") || "kubeconfig"}.</div>
            </Centered>
          </Match>
          <Match when={state.connectError}>
            {(err) => (
              <Centered>
                <ErrorNotice
                  error={err()}
                  title={`Cannot connect to ${state.context}`}
                  onRetry={() => state.context && void session.connect(state.context)}
                />
                <div class="mt-3 text-muted">You can pick another context in the sidebar.</div>
              </Centered>
            )}
          </Match>
          <Match when={state.connecting}>
            <Centered>
              <div class="text-muted">Connecting to {state.context}…</div>
            </Centered>
          </Match>
          <Match when={viewKey()}>
            <Show when={viewKey()} keyed>
              {(_key) => (
                <ResourceView
                  context={state.context!}
                  kind={state.kind}
                  namespace={effectiveNamespace()}
                  title={session.kindInfo()?.title ?? state.kind}
                />
              )}
            </Show>
          </Match>
        </Switch>
      </main>
    </div>
  );
}

function Centered(props: { children: import("solid-js").JSX.Element }) {
  return (
    <div class="flex h-full items-center justify-center p-8">
      <div class="w-full max-w-xl">{props.children}</div>
    </div>
  );
}
