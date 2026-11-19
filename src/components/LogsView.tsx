// Pod logs: Rust-side store, virtualized viewer, search, time modes, export.

import { createVirtualizer } from "@tanstack/solid-virtual";
import { For, Show, batch, createEffect, createMemo, createSignal, on, onCleanup, untrack } from "solid-js";
import { api, toAppError, type Subscription } from "../lib/api";
import { searchRegExp, segmentLine } from "../lib/logText";
import type { AppError, ContainerRef, LogLine, LogQuery, LogStatus, TimeMode } from "../lib/types";
import { WindowCache } from "../lib/windowCache";
import { DownloadPanel } from "./DownloadPanel";
import { ErrorNotice } from "./ErrorNotice";

const LINE_HEIGHT = 18;
// Container names are DNS labels, so "*" can never collide with one.
const ALL = "*";
const QUERY_DEBOUNCE_MS = 150;

export const HISTORY = [
  { key: "tail100", label: "Last 100 lines", tailLines: 100, sinceSeconds: null },
  { key: "tail1000", label: "Last 1,000 lines", tailLines: 1000, sinceSeconds: null },
  { key: "tail10000", label: "Last 10,000 lines", tailLines: 10000, sinceSeconds: null },
  { key: "since5m", label: "Last 5 minutes", tailLines: null, sinceSeconds: 300 },
  { key: "since1h", label: "Last hour", tailLines: null, sinceSeconds: 3600 },
  { key: "since24h", label: "Last 24 hours", tailLines: null, sinceSeconds: 86400 },
  { key: "all", label: "Everything", tailLines: null, sinceSeconds: null },
] as const;
type HistoryKey = (typeof HISTORY)[number]["key"];

const TIME_MODES: { mode: TimeMode; label: string; title: string }[] = [
  { mode: "none", label: "No time", title: "Hide timestamps" },
  { mode: "local", label: "Local", title: "Timestamps in your time zone" },
  { mode: "utc", label: "UTC", title: "Timestamps in UTC" },
];

// Distinct, readable colors for container tags in merged views.
const CONTAINER_COLORS = ["#2563eb", "#c026d3", "#0891b2", "#ca8a04", "#16a34a", "#dc2626", "#7c3aed", "#db2777"];

interface Prefs {
  timeMode: TimeMode;
  wrap: boolean;
  lineNumbers: boolean;
  history: HistoryKey;
}

const PREFS_KEY = "kubeviewer:logPrefs";
function loadPrefs(): Prefs {
  const defaults: Prefs = { timeMode: "local", wrap: false, lineNumbers: true, history: "tail1000" };
  try {
    const saved = JSON.parse(window.localStorage.getItem(PREFS_KEY) ?? "{}") as Partial<Prefs>;
    return {
      timeMode: TIME_MODES.some((t) => t.mode === saved.timeMode) ? saved.timeMode! : defaults.timeMode,
      wrap: typeof saved.wrap === "boolean" ? saved.wrap : defaults.wrap,
      lineNumbers: typeof saved.lineNumbers === "boolean" ? saved.lineNumbers : defaults.lineNumbers,
      history: HISTORY.some((h) => h.key === saved.history) ? saved.history! : defaults.history,
    };
  } catch {
    return defaults;
  }
}
function savePrefs(prefs: Prefs) {
  try {
    window.localStorage.setItem(PREFS_KEY, JSON.stringify(prefs));
  } catch {
    // Storage unavailable: preferences just aren't remembered.
  }
}

export interface LogsViewProps {
  context: string;
  namespace: string;
  pod: string;
  containers: ContainerRef[];
}

export function LogsView(props: LogsViewProps) {
  const prefs = loadPrefs();
  const mainContainers = () => props.containers.filter((c) => !c.init).map((c) => c.name);
  const [container, setContainer] = createSignal<string>(mainContainers()[0] ?? props.containers[0]?.name ?? "");
  const [history, setHistory] = createSignal<HistoryKey>(prefs.history);
  const [previous, setPrevious] = createSignal(false);
  const [timeMode, setTimeMode] = createSignal<TimeMode>(prefs.timeMode);
  const [wrap, setWrap] = createSignal(prefs.wrap);
  const [lineNumbers, setLineNumbers] = createSignal(prefs.lineNumbers);
  const [autoScroll, setAutoScroll] = createSignal(true);
  const [unseenFrom, setUnseenFrom] = createSignal(0);

  const [queryText, setQueryText] = createSignal("");
  const [regex, setRegex] = createSignal(false);
  const [caseSensitive, setCaseSensitive] = createSignal(false);
  const [queryMode, setQueryMode] = createSignal<"filter" | "find">("find");
  const [queryError, setQueryError] = createSignal<string | null>(null);
  const [current, setCurrent] = createSignal<{ index: number; ordinal: number } | null>(null);

  const [status, setStatus] = createSignal<LogStatus | null>(null);
  const [openError, setOpenError] = createSignal<AppError | null>(null);
  const [containersInView, setContainersInView] = createSignal<string[]>([]);
  const [sessionId, setSessionId] = createSignal<number | null>(null);
  const [total, setTotal] = createSignal(0);
  const [revision, setRevision] = createSignal(0);
  const [showDownload, setShowDownload] = createSignal(false);

  createEffect(() => savePrefs({ timeMode: timeMode(), wrap: wrap(), lineNumbers: lineNumbers(), history: history() }));

  let scrollEl: HTMLDivElement | undefined;
  let searchInput: HTMLInputElement | undefined;

  const cache = new WindowCache<LogLine>(
    async (start, end) => {
      const id = untrack(sessionId);
      if (id === null) return { version: 0, total: 0, start, items: [] };
      const w = await api.getLogLines(id, start, end, untrack(timeMode));
      return { version: w.version, total: w.total, start: w.start, items: w.lines };
    },
    (t) => {
      setTotal(t);
      setRevision((r) => r + 1);
    },
    (error) => setOpenError(toAppError(error)),
    { overscan: 60 },
  );
  onCleanup(() => cache.dispose());

  // The query the backend currently has; find() must never run against a stale one.
  let appliedQuery: LogQuery | null = null;
  const sameQuery = (a: LogQuery | null, b: LogQuery | null) =>
    a === b || (!!a && !!b && a.text === b.text && a.regex === b.regex && a.caseSensitive === b.caseSensitive && a.mode === b.mode);

  const query = createMemo<LogQuery | null>(
    () => (queryText() ? { text: queryText(), regex: regex(), caseSensitive: caseSensitive(), mode: queryMode() } : null),
    null,
    { equals: (a, b) => sameQuery(a, b) },
  );
  const highlight = createMemo(() => searchRegExp(query()));

  const virtualizer = createVirtualizer({
    get count() {
      return total();
    },
    getScrollElement: () => scrollEl ?? null,
    estimateSize: () => LINE_HEIGHT,
    overscan: 20,
  });

  createEffect(() => {
    const items = virtualizer.getVirtualItems();
    const first = items[0];
    const last = items[items.length - 1];
    cache.setVisible(first?.index ?? 0, last ? last.index + 1 : 0);
  });

  // Row heights change with wrapping; drop measurements when it toggles.
  createEffect(on(wrap, () => virtualizer.measure(), { defer: true }));
  // Timestamps are formatted by the backend: refetch on mode change.
  createEffect(on(timeMode, () => cache.reset(), { defer: true }));

  // Open a session whenever what we stream changes.
  createEffect(
    on([container, history, previous], ([selected, historyKey, prev]) => {
      const h = HISTORY.find((x) => x.key === historyKey) ?? HISTORY[1];
      const containers = selected === ALL ? mainContainers() : [selected];
      let sub: Subscription | null = null;
      let disposed = false;
      appliedQuery = null;
      batch(() => {
        setSessionId(null);
        setStatus(null);
        setOpenError(null);
        setCurrent(null);
        setTotal(0);
        setAutoScroll(true);
      });
      cache.reset();
      api
        .openLogs(
          {
            context: props.context,
            namespace: props.namespace,
            pod: props.pod,
            request: { containers, tailLines: h.tailLines, sinceSeconds: h.sinceSeconds, previous: prev, follow: !prev },
          },
          (s) => {
            if (disposed) return;
            setStatus(s);
            setTotal(s.visible);
            cache.invalidate(s.version);
          },
        )
        .then(async (opened) => {
          if (disposed) {
            opened.close();
            return;
          }
          sub = opened;
          batch(() => {
            setSessionId(opened.id);
            setContainersInView(opened.containers);
          });
          const q = untrack(query);
          if (q) await applyQuery(q);
          cache.reset();
        })
        .catch((e) => !disposed && setOpenError(toAppError(e)));
      onCleanup(() => {
        disposed = true;
        sub?.close();
      });
    }),
  );

  async function applyQuery(q: LogQuery | null) {
    const id = sessionId();
    if (id === null) return;
    try {
      const result = await api.setLogQuery(id, q);
      appliedQuery = q;
      batch(() => {
        setQueryError(null);
        setCurrent(null);
        setTotal(result.visible);
        setStatus((s) => (s ? { ...s, matches: result.matches, visible: result.visible } : s));
      });
      cache.invalidate(result.version);
    } catch (e) {
      setQueryError(toAppError(e).message);
    }
  }

  let queryTimer: number | undefined;
  createEffect(
    on(
      query,
      (q) => {
        window.clearTimeout(queryTimer);
        queryTimer = window.setTimeout(() => void applyQuery(q), QUERY_DEBOUNCE_MS);
      },
      { defer: true },
    ),
  );
  onCleanup(() => window.clearTimeout(queryTimer));

  async function find(forward: boolean) {
    const id = sessionId();
    if (id === null || !queryText()) return;
    if (!sameQuery(appliedQuery, query())) {
      window.clearTimeout(queryTimer);
      await applyQuery(query());
    }
    let from: number | null = current()?.index ?? null;
    if (from === null) {
      const items = virtualizer.getVirtualItems();
      const first = items[0]?.index ?? 0;
      const last = items[items.length - 1]?.index ?? 0;
      from = forward ? (first > 0 ? first - 1 : null) : last + 1 < total() ? last + 1 : null;
    }
    try {
      const result = await api.findLogMatch(id, from, forward);
      if (!result) return;
      pauseFollow();
      setCurrent({ index: result.index, ordinal: result.ordinal });
      virtualizer.scrollToIndex(result.index, { align: "center" });
    } catch (e) {
      setQueryError(toAppError(e).message);
    }
  }

  // Keep the newest line in view while auto-scroll is on.
  createEffect(
    on(total, (n) => {
      if (untrack(autoScroll) && n > 0) virtualizer.scrollToIndex(n - 1, { align: "end" });
    }),
  );

  let lastScrollTop = 0;
  function onScroll() {
    if (!scrollEl) return;
    const atBottom = scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight < LINE_HEIGHT * 2;
    if (atBottom) {
      setAutoScroll(true);
    } else if (scrollEl.scrollTop < lastScrollTop) {
      // The reader scrolled up: stop following and count what arrives meanwhile.
      pauseFollow();
    }
    lastScrollTop = scrollEl.scrollTop;
  }

  /** Stops following the tail; lines arriving from now on count as new. */
  function pauseFollow() {
    if (!autoScroll()) return;
    setAutoScroll(false);
    setUnseenFrom(total());
  }

  function jumpToEnd() {
    setAutoScroll(true);
    if (total() > 0) virtualizer.scrollToIndex(total() - 1, { align: "end" });
  }

  function onKeyDown(e: KeyboardEvent) {
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.key.toLowerCase() === "f") {
      e.preventDefault();
      searchInput?.focus();
      searchInput?.select();
    } else if (mod && e.key === "ArrowDown") {
      e.preventDefault();
      jumpToEnd();
    } else if (mod && e.key === "ArrowUp") {
      e.preventDefault();
      pauseFollow();
      virtualizer.scrollToIndex(0, { align: "start" });
    }
  }

  const multi = () => containersInView().length > 1;
  const lineAt = (index: number) => {
    void revision();
    return cache.get(index);
  };
  const phaseLabel = () => {
    const s = status();
    if (!s) return "connecting…";
    return { loading: "loading history…", following: "live", reconnecting: "reconnecting…", ended: "ended" }[s.phase];
  };
  const newLines = () => (autoScroll() ? 0 : Math.max(0, total() - unseenFrom()));
  const filtering = () => queryMode() === "filter" && !!queryText();

  return (
    <div class="flex h-full flex-col" onKeyDown={onKeyDown}>
      {/* Row 1: what to stream and how to show it */}
      <div class="flex flex-wrap items-center gap-2 border-b border-line px-2 py-1.5 text-[12px]">
        <Show when={props.containers.length > 1}>
          <select aria-label="Container" class="rounded border border-line bg-canvas px-1 py-0.5" onChange={(e) => setContainer(e.currentTarget.value)}>
            <Show when={mainContainers().length > 1}>
              <option value={ALL} selected={container() === ALL}>
                All containers
              </option>
            </Show>
            <For each={props.containers}>
              {(c) => (
                <option value={c.name} selected={c.name === container()}>
                  {c.name}
                  {c.init ? " (init)" : ""}
                </option>
              )}
            </For>
          </select>
        </Show>
        <select aria-label="History" class="rounded border border-line bg-canvas px-1 py-0.5" onChange={(e) => setHistory(e.currentTarget.value as HistoryKey)}>
          <For each={HISTORY}>
            {(h) => (
              <option value={h.key} selected={h.key === history()}>
                {h.label}
              </option>
            )}
          </For>
        </select>
        <Toggle label="Previous" title="Logs of the previous (crashed) container instance" value={previous()} onChange={setPrevious} />
        <Segmented
          label="Timestamps"
          options={TIME_MODES.map((t) => ({ value: t.mode, label: t.label, title: t.title }))}
          value={timeMode()}
          onChange={setTimeMode}
        />
        <Toggle label="Wrap" value={wrap()} onChange={setWrap} />
        <Toggle label="Line #" value={lineNumbers()} onChange={setLineNumbers} />
        <div class="flex-1" />
        <span class="tabular-nums text-muted" aria-live="polite">
          <Show when={status()} fallback={phaseLabel()}>
            {(s) => (
              <>
                {total().toLocaleString()}
                <Show when={filtering()}> of {s().lines.toLocaleString()}</Show> lines
                <Show when={s().dropped > 0}>
                  <span title="Older lines were dropped from memory. Download gets the full log.">
                    {" "}
                    · {s().dropped.toLocaleString()} older dropped
                  </span>
                </Show>
                {" · "}
                <span classList={{ "text-ok": s().phase === "following", "text-warn": s().phase === "reconnecting" }}>{phaseLabel()}</span>
              </>
            )}
          </Show>
        </span>
        <button
          class="rounded border border-line px-2 py-0.5 hover:bg-hover"
          aria-expanded={showDownload()}
          onClick={() => setShowDownload((v) => !v)}
        >
          Download…
        </button>
      </div>

      {/* Row 2: search */}
      <div class="flex items-center gap-2 border-b border-line px-2 py-1 text-[12px]">
        <Segmented
          label="Search mode"
          options={[
            { value: "find", label: "Find", title: "Highlight matches and jump between them" },
            { value: "filter", label: "Filter", title: "Show only matching lines" },
          ]}
          value={queryMode()}
          onChange={setQueryMode}
        />
        <input
          ref={searchInput}
          type="search"
          aria-label="Search logs"
          placeholder="Search logs (⌘F) · Enter next · ⇧Enter previous"
          class="min-w-0 flex-1 rounded border bg-canvas px-2 py-0.5"
          classList={{ "border-error": !!queryError(), "border-line": !queryError() }}
          value={queryText()}
          onInput={(e) => setQueryText(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void find(!e.shiftKey);
            } else if (e.key === "Escape") {
              setQueryText("");
            }
          }}
        />
        <Toggle label=".*" title="Regular expression" value={regex()} onChange={setRegex} />
        <Toggle label="Aa" title="Match case" value={caseSensitive()} onChange={setCaseSensitive} />
        <Show when={queryText() && status()}>
          <span class="tabular-nums text-muted" aria-live="polite">
            <Show when={current()} fallback={`${(status()?.matches ?? 0).toLocaleString()} matches`}>
              {(c) => (
                <>
                  {c().ordinal.toLocaleString()} / {(status()?.matches ?? 0).toLocaleString()}
                </>
              )}
            </Show>
          </span>
          <button aria-label="Previous match" class="rounded px-1.5 hover:bg-hover" onClick={() => void find(false)}>
            ↑
          </button>
          <button aria-label="Next match" class="rounded px-1.5 hover:bg-hover" onClick={() => void find(true)}>
            ↓
          </button>
        </Show>
      </div>
      <Show when={queryError()}>
        <div role="alert" class="border-b border-line bg-error-bg px-2 py-1 font-mono text-[12px] text-error">
          {queryError()}
        </div>
      </Show>

      <Show when={showDownload()}>
        <DownloadPanel
          context={props.context}
          namespace={props.namespace}
          pod={props.pod}
          containers={containersInView()}
          previous={previous()}
          timeMode={timeMode()}
          filtered={filtering()}
          loadedLines={total()}
          sessionId={sessionId()}
          onClose={() => setShowDownload(false)}
        />
      </Show>

      <Show when={openError() ?? status()?.error}>
        {(e) => (
          <div class="p-2">
            <ErrorNotice
              compact
              error={e()}
              title={status()?.phase === "reconnecting" ? "Log stream interrupted — retrying" : "Cannot read logs"}
            />
          </div>
        )}
      </Show>

      <div class="relative min-h-0 flex-1">
        <div
          ref={scrollEl}
          class="selectable absolute inset-0 overflow-auto font-mono text-[12px] leading-[18px]"
          onScroll={onScroll}
          tabIndex={0}
          aria-label="Log lines"
        >
          <div
            style={{
              height: `${virtualizer.getTotalSize()}px`,
              position: "relative",
              "min-width": "100%",
              width: wrap() ? "100%" : "max-content",
            }}
          >
            <For each={virtualizer.getVirtualItems()}>
              {(item) => {
                const l = createMemo(() => lineAt(item.index));
                let rowEl: HTMLDivElement | undefined;
                // Wrapped rows have variable height: measure after content changes.
                createEffect(
                  on([wrap, l], ([wrapped]) => {
                    if (wrapped && rowEl) queueMicrotask(() => rowEl?.isConnected && virtualizer.measureElement(rowEl));
                  }),
                );
                return (
                  <div
                    data-index={item.index}
                    data-line={l()?.n}
                    ref={rowEl}
                    class="absolute left-0 top-0 flex px-2"
                    classList={{ "w-full": wrap(), "min-w-full": !wrap(), "bg-selected": current()?.index === item.index }}
                    style={{
                      transform: `translateY(${item.start}px)`,
                      "min-height": `${LINE_HEIGHT}px`,
                      height: wrap() ? undefined : `${LINE_HEIGHT}px`,
                    }}
                  >
                    <Show when={l()} fallback={<span class="text-muted">…</span>}>
                      {(ln) => (
                        <>
                          <Show when={lineNumbers()}>
                            <span class="w-[8ch] shrink-0 select-none pr-3 text-right text-muted/70">{ln().n}</span>
                          </Show>
                          <Show when={ln().ts}>
                            <span class="shrink-0 pr-3 text-muted">{ln().ts}</span>
                          </Show>
                          <Show when={multi()}>
                            <span class="shrink-0 pr-2" style={{ color: CONTAINER_COLORS[ln().c % CONTAINER_COLORS.length] }}>
                              {containersInView()[ln().c]}
                            </span>
                          </Show>
                          <span classList={{ "min-w-0 whitespace-pre-wrap break-all": wrap(), "whitespace-pre": !wrap() }}>
                            <For each={segmentLine(ln().text, highlight())}>
                              {(seg) => (
                                <span class={seg.cls} style={{ color: seg.color, "background-color": seg.background }}>
                                  {seg.text}
                                </span>
                              )}
                            </For>
                          </span>
                        </>
                      )}
                    </Show>
                  </div>
                );
              }}
            </For>
          </div>
          <Show when={status() && total() === 0 && status()!.phase !== "loading"}>
            <div class="pointer-events-none absolute inset-0 flex items-center justify-center text-muted">
              {filtering() ? "No matching lines" : "No log lines yet"}
            </div>
          </Show>
        </div>
        <Show when={!autoScroll() && total() > 0}>
          <button
            class="absolute bottom-3 right-4 rounded-full px-3 py-1 text-[12px] shadow"
            classList={{ "bg-accent text-accent-fg": newLines() > 0, "border border-line bg-canvas hover:bg-hover": newLines() === 0 }}
            onClick={jumpToEnd}
          >
            <Show when={newLines() > 0} fallback="↓ Jump to end">
              ↓ {newLines().toLocaleString()} new {newLines() === 1 ? "line" : "lines"}
            </Show>
          </button>
        </Show>
      </div>
    </div>
  );
}

function Toggle(props: { label: string; title?: string; value: boolean; onChange: (v: boolean) => void }) {
  return (
    <label class="flex items-center gap-1" title={props.title}>
      <input type="checkbox" checked={props.value} onChange={(e) => props.onChange(e.currentTarget.checked)} />
      {props.label}
    </label>
  );
}

export function Segmented<T extends string>(props: {
  label: string;
  options: { value: T; label: string; title?: string }[];
  value: T;
  onChange: (value: T) => void;
}) {
  return (
    <div role="radiogroup" aria-label={props.label} class="flex overflow-hidden rounded border border-line">
      <For each={props.options}>
        {(o) => (
          <button
            role="radio"
            aria-checked={props.value === o.value}
            title={o.title}
            class="px-2 py-0.5"
            classList={{ "bg-accent text-accent-fg": props.value === o.value, "hover:bg-hover": props.value !== o.value }}
            onClick={() => props.onChange(o.value)}
          >
            {o.label}
          </button>
        )}
      </For>
    </div>
  );
}
