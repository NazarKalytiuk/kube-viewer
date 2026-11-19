// One table view (context × kind × namespace) plus the detail panel.

import { Show, createSignal, onCleanup, onMount } from "solid-js";
import { api, toAppError, type Subscription } from "../lib/api";
import { RowWindow } from "../lib/rowWindow";
import type { AppError, Column, ResourceKind, Row, SortSpec, ViewUpdate } from "../lib/types";
import { session } from "../stores/session";
import { DetailPanel } from "./DetailPanel";
import { ErrorNotice } from "./ErrorNotice";
import { ResourceTable } from "./ResourceTable";

const FILTER_DEBOUNCE_MS = 120;

export function ResourceView(props: { context: string; kind: ResourceKind; namespace: string | null; title: string }) {
  const [columns, setColumns] = createSignal<Column[]>([]);
  const [sort, setSort] = createSignal<SortSpec>({ column: "name", descending: false });
  const [update, setUpdate] = createSignal<ViewUpdate | null>(null);
  // Counts come from two sources (watch updates and query results); keep
  // them in signals of their own so the header always shows the latest.
  const [total, setTotal] = createSignal(0);
  const [unfiltered, setUnfiltered] = createSignal(0);
  const [revision, setRevision] = createSignal(0);
  const [openError, setOpenError] = createSignal<AppError | null>(null);
  const [filter, setFilter] = createSignal("");
  const [detailWidth, setDetailWidth] = createSignal(loadDetailWidth());

  let viewId: number | null = null;
  let sub: Subscription | null = null;
  let disposed = false;
  let filterTimer: number | undefined;

  const rows = new RowWindow(
    (start, end) => {
      if (viewId === null) return Promise.resolve({ version: 0, total: 0, start, rows: [] });
      return api.getRows(viewId, start, end);
    },
    (t) => {
      setTotal(t);
      setRevision((r) => r + 1);
    },
    (error) => setOpenError(toAppError(error)),
  );

  onMount(async () => {
    try {
      const opened = await api.openView(
        { context: props.context, kind: props.kind, namespace: props.namespace, sort: null, filter: "" },
        (u) => {
          setUpdate(u);
          setTotal(u.total);
          setUnfiltered(u.unfiltered);
          rows.invalidate(u.version);
        },
      );
      if (disposed) {
        opened.close();
        return;
      }
      sub = opened;
      viewId = opened.id;
      setColumns(opened.columns);
      setSort(opened.sort);
    } catch (error) {
      if (!disposed) setOpenError(toAppError(error));
    }
  });

  onCleanup(() => {
    disposed = true;
    window.clearTimeout(filterTimer);
    sub?.close();
    rows.dispose();
  });

  async function applyQuery(nextSort: SortSpec, nextFilter: string) {
    if (viewId === null) return;
    try {
      const result = await api.setViewQuery(viewId, nextSort, nextFilter);
      setSort(result.sort);
      setTotal(result.total);
      rows.invalidate(result.version);
    } catch (error) {
      setOpenError(toAppError(error));
    }
  }

  function onSort(column: string) {
    const current = sort();
    const next = { column, descending: current.column === column ? !current.descending : false };
    void applyQuery(next, filter());
  }

  function onFilter(value: string) {
    setFilter(value);
    window.clearTimeout(filterTimer);
    filterTimer = window.setTimeout(() => void applyQuery(sort(), value), FILTER_DEBOUNCE_MS);
  }

  function onSelect(row: Row | null) {
    session.select(row ? { uid: row.uid, name: row.name, namespace: row.namespace } : null);
  }

  const loading = () => update()?.phase !== "ready";
  const watchError = () => update()?.error ?? null;

  return (
    <div class="flex h-full min-w-0 flex-1">
      <section class="flex min-w-0 flex-1 flex-col">
        <header class="flex items-center gap-3 border-b border-line px-3 py-2">
          <h1 class="text-[15px] font-semibold">{props.title}</h1>
          <span class="tabular-nums text-muted" aria-live="polite">
            <Show when={update()} fallback="…">
              {total().toLocaleString()}
              <Show when={total() !== unfiltered()}> of {unfiltered().toLocaleString()}</Show>
              <Show when={loading()}> · loading</Show>
            </Show>
          </span>
          <div class="flex-1" />
          <input
            type="search"
            aria-label="Filter"
            placeholder="Filter by name, namespace, status…"
            class="w-72 rounded border border-line bg-canvas px-2 py-1"
            value={filter()}
            onInput={(e) => onFilter(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") onFilter("");
            }}
          />
        </header>

        <Show when={openError()}>
          {(err) => (
            <div class="p-3">
              <ErrorNotice error={err()} title={`Cannot open ${props.title}`} />
            </div>
          )}
        </Show>
        <Show when={watchError()}>
          {(err) => (
            <div class="px-3 pt-2">
              <ErrorNotice compact error={err()} title="Watch interrupted — retrying" />
            </div>
          )}
        </Show>

        <div class="min-h-0 flex-1">
          <Show when={columns().length}>
            <ResourceTable
              columns={columns()}
              total={total()}
              revision={revision()}
              rows={rows}
              sort={sort()}
              onSort={onSort}
              selectedUid={session.state.selection?.uid ?? null}
              onSelect={onSelect}
            />
          </Show>
        </div>
      </section>

      <Show when={session.state.selection} keyed>
        {(selection) => (
          <>
            <Splitter width={detailWidth()} onResize={setDetailWidth} />
            <div class="h-full shrink-0 border-l border-line" style={{ width: `${detailWidth()}px` }}>
              <DetailPanel
                context={props.context}
                kind={props.kind}
                namespace={selection.namespace}
                name={selection.name}
                onClose={() => session.select(null)}
              />
            </div>
          </>
        )}
      </Show>
    </div>
  );
}

function loadDetailWidth(): number {
  try {
    const saved = Number(window.localStorage.getItem("kubeviewer:detailWidth"));
    if (saved >= 320 && saved <= 1600) return saved;
  } catch {
    // ignore
  }
  return 560;
}

function Splitter(props: { width: number; onResize: (width: number) => void }) {
  function onPointerDown(e: PointerEvent) {
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = props.width;
    const target = e.currentTarget as HTMLElement;
    target.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => {
      const width = Math.min(Math.max(320, startWidth + (startX - ev.clientX)), window.innerWidth - 420);
      props.onResize(width);
    };
    const up = () => {
      target.removeEventListener("pointermove", move);
      target.removeEventListener("pointerup", up);
      try {
        window.localStorage.setItem("kubeviewer:detailWidth", String(props.width));
      } catch {
        // ignore
      }
    };
    target.addEventListener("pointermove", move);
    target.addEventListener("pointerup", up);
  }
  return (
    <div
      role="separator"
      aria-orientation="vertical"
      class="w-1 shrink-0 cursor-col-resize bg-transparent hover:bg-accent/40"
      onPointerDown={onPointerDown}
    />
  );
}
