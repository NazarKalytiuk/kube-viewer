// Virtualized resource table. Only the visible rows (plus overscan) exist in
// the DOM and in memory; row data is pulled from the Rust view on demand.

import { createVirtualizer } from "@tanstack/solid-virtual";
import { For, Show, createEffect, createMemo, on } from "solid-js";
import { cellText, nowSeconds } from "../lib/format";
import type { RowWindow } from "../lib/rowWindow";
import type { Cell, Column, Row, SortSpec, Tone } from "../lib/types";

export const ROW_HEIGHT = 26;

const toneClass: Record<Tone, string> = {
  ok: "text-ok",
  warn: "text-warn",
  error: "text-error font-medium",
  muted: "text-muted",
};

export interface ResourceTableProps {
  columns: Column[];
  total: number;
  /** Bumped whenever the row cache changes. */
  revision: number;
  rows: RowWindow;
  sort: SortSpec;
  onSort: (column: string) => void;
  selectedUid: string | null;
  onSelect: (row: Row | null) => void;
  /** Index of the row to scroll into view (keyboard navigation, restored selection). */
  scrollTo?: number | null;
}

export function ResourceTable(props: ResourceTableProps) {
  let scrollEl: HTMLDivElement | undefined;

  const template = createMemo(() => props.columns.map((c, i) => (i === props.columns.length - 1 ? `minmax(${c.width}px, 1fr)` : `${c.width}px`)).join(" "));
  const minWidth = createMemo(() => props.columns.reduce((sum, c) => sum + c.width, 0));

  const virtualizer = createVirtualizer({
    get count() {
      return props.total;
    },
    getScrollElement: () => scrollEl ?? null,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
    // Header is sticky inside the scroll container.
    paddingStart: ROW_HEIGHT,
  });

  createEffect(() => {
    const items = virtualizer.getVirtualItems();
    const first = items[0];
    const last = items[items.length - 1];
    if (first && last) props.rows.setVisible(first.index, last.index + 1);
    else props.rows.setVisible(0, 0);
  });

  createEffect(
    on(
      () => props.scrollTo,
      (index) => {
        if (index != null && index >= 0) virtualizer.scrollToIndex(index, { align: "auto" });
      },
    ),
  );

  // Keyboard navigation can target a row that is not cached yet (End,
  // PageDown); it is selected as soon as its data arrives.
  let pendingSelect: number | null = null;
  createEffect(
    on(
      () => props.revision,
      () => {
        if (pendingSelect === null) return;
        const row = props.rows.get(pendingSelect);
        if (row) {
          pendingSelect = null;
          props.onSelect(row);
        }
      },
    ),
  );

  const rowAt = (index: number) => {
    void props.revision;
    return props.rows.get(index);
  };

  function selectIndex(index: number) {
    if (props.total === 0) return;
    const next = Math.min(props.total - 1, Math.max(0, index));
    virtualizer.scrollToIndex(next, { align: "auto" });
    const row = props.rows.get(next);
    if (row) {
      pendingSelect = null;
      props.onSelect(row);
    } else {
      pendingSelect = next;
    }
  }

  /** Index of the selected row if it is in the rendered range. */
  function selectedIndex(): number | null {
    if (!props.selectedUid) return null;
    for (const item of virtualizer.getVirtualItems()) {
      if (props.rows.get(item.index)?.uid === props.selectedUid) return item.index;
    }
    return null;
  }

  function moveSelection(delta: number) {
    const current = selectedIndex();
    const firstVisible = virtualizer.getVirtualItems()[0]?.index ?? 0;
    selectIndex(current === null ? firstVisible : current + delta);
  }

  function onKeyDown(e: KeyboardEvent) {
    const page = Math.max(1, Math.floor((scrollEl?.clientHeight ?? ROW_HEIGHT * 10) / ROW_HEIGHT) - 1);
    const keys: Record<string, number> = { ArrowDown: 1, ArrowUp: -1, PageDown: page, PageUp: -page };
    if (e.key in keys) {
      e.preventDefault();
      moveSelection(keys[e.key]!);
    } else if (e.key === "Home") {
      e.preventDefault();
      selectIndex(0);
    } else if (e.key === "End") {
      e.preventDefault();
      selectIndex(props.total - 1);
    } else if (e.key === "Escape") {
      props.onSelect(null);
    }
  }

  return (
    <div
      ref={scrollEl}
      class="relative h-full overflow-auto outline-none"
      tabIndex={0}
      role="grid"
      aria-rowcount={props.total}
      onKeyDown={onKeyDown}
    >
      <div style={{ height: `${virtualizer.getTotalSize()}px`, "min-width": `${minWidth()}px`, position: "relative" }}>
        <div
          role="row"
          class="sticky top-0 z-10 grid border-b border-line bg-raised text-[12px] font-medium text-muted"
          style={{ "grid-template-columns": template(), height: `${ROW_HEIGHT}px` }}
        >
          <For each={props.columns}>
            {(col) => (
              <button
                role="columnheader"
                class="flex items-center gap-1 truncate px-2 text-left hover:text-fg"
                classList={{ "justify-end": col.kind === "number" || col.kind === "age" }}
                aria-sort={props.sort.column === col.id ? (props.sort.descending ? "descending" : "ascending") : "none"}
                onClick={() => props.onSort(col.id)}
              >
                <span class="truncate">{col.title}</span>
                <Show when={props.sort.column === col.id}>
                  <span aria-hidden="true">{props.sort.descending ? "▾" : "▴"}</span>
                </Show>
              </button>
            )}
          </For>
        </div>

        <For each={virtualizer.getVirtualItems()}>
          {(item) => {
            const row = createMemo(() => rowAt(item.index));
            return (
              <div
                role="row"
                aria-rowindex={item.index + 1}
                aria-selected={row()?.uid === props.selectedUid}
                class="vrow absolute top-0 left-0 grid w-full cursor-default border-b border-line/50"
                classList={{
                  "bg-selected": !!row() && row()!.uid === props.selectedUid,
                  "hover:bg-hover": !row() || row()!.uid !== props.selectedUid,
                }}
                style={{
                  transform: `translateY(${item.start}px)`,
                  height: `${ROW_HEIGHT}px`,
                  "grid-template-columns": template(),
                }}
                onMouseDown={() => {
                  const r = row();
                  if (r) props.onSelect(r);
                }}
              >
                <Show when={row()} fallback={<Placeholder columns={props.columns.length} />}>
                  {(r) => (
                    <For each={props.columns}>
                      {(col, i) => <TableCell cell={r().cells[i()] ?? null} column={col} />}
                    </For>
                  )}
                </Show>
              </div>
            );
          }}
        </For>
      </div>
    </div>
  );
}

function TableCell(props: { cell: Cell; column: Column }) {
  const text = () => cellText(props.cell, props.column.kind, props.column.kind === "age" ? nowSeconds() : 0);
  const tone = () => (props.cell !== null && typeof props.cell === "object" ? toneClass[props.cell.tone] : "");
  return (
    <div
      role="gridcell"
      class={`truncate px-2 leading-[26px] ${tone()}`}
      classList={{ "text-right tabular-nums": props.column.kind === "number" || props.column.kind === "age" }}
      title={text()}
    >
      {text()}
    </div>
  );
}

function Placeholder(props: { columns: number }) {
  return (
    <For each={Array.from({ length: props.columns })}>
      {() => (
        <div class="flex items-center px-2">
          <div class="h-2 w-2/3 rounded bg-raised" />
        </div>
      )}
    </For>
  );
}
