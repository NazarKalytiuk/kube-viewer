// Streaming pod logs with a bounded, virtualized line buffer.

import { createVirtualizer } from "@tanstack/solid-virtual";
import { For, Show, createEffect, createMemo, createSignal, on, onCleanup } from "solid-js";
import { api, toAppError, type Subscription } from "../lib/api";
import { LogBuffer } from "../lib/logBuffer";
import type { AppError, ContainerRef } from "../lib/types";
import { ErrorNotice } from "./ErrorNotice";

const LINE_HEIGHT = 18;
const TAIL_CHOICES: { label: string; value: number | null }[] = [
  { label: "Last 100", value: 100 },
  { label: "Last 1,000", value: 1000 },
  { label: "Last 10,000", value: 10000 },
  { label: "All", value: null },
];

export function LogsView(props: { context: string; namespace: string; pod: string; containers: ContainerRef[] }) {
  const firstContainer = () => props.containers.find((c) => !c.init)?.name ?? props.containers[0]?.name ?? null;
  const [container, setContainer] = createSignal<string | null>(firstContainer());
  const [tail, setTail] = createSignal<number | null>(1000);
  const [timestamps, setTimestamps] = createSignal(false);
  const [previous, setPrevious] = createSignal(false);
  const [follow, setFollow] = createSignal(true);
  const [filter, setFilter] = createSignal("");
  const [status, setStatus] = createSignal<"streaming" | "ended">("streaming");
  const [error, setError] = createSignal<AppError | null>(null);
  const [revision, setRevision] = createSignal(0);

  const buffer = new LogBuffer();
  let scrollEl: HTMLDivElement | undefined;
  let stickToBottom = true;

  const count = createMemo(() => {
    void revision();
    return buffer.visibleCount;
  });

  const virtualizer = createVirtualizer({
    get count() {
      return count();
    },
    getScrollElement: () => scrollEl ?? null,
    estimateSize: () => LINE_HEIGHT,
    overscan: 30,
  });

  // (Re)subscribe whenever stream options change.
  createEffect(() => {
    const options = {
      container: container(),
      tailLines: tail(),
      timestamps: timestamps(),
      previous: previous(),
      follow: true,
    };
    buffer.clear();
    stickToBottom = true;
    setRevision((r) => r + 1);
    setStatus("streaming");
    setError(null);

    let sub: Subscription | null = null;
    let disposed = false;
    api
      .openLogs({ context: props.context, namespace: props.namespace, pod: props.pod, options }, (event) => {
        if (event.type === "lines") {
          buffer.append(event.lines);
          setRevision((r) => r + 1);
        } else if (event.type === "end") {
          setStatus("ended");
        } else {
          setError(event.error);
          setStatus("ended");
        }
      })
      .then((s) => {
        if (disposed) s.close();
        else sub = s;
      })
      .catch((e) => !disposed && setError(toAppError(e)));
    onCleanup(() => {
      disposed = true;
      sub?.close();
    });
  });

  createEffect(
    on(filter, (f) => {
      buffer.setFilter(f);
      setRevision((r) => r + 1);
    }),
  );

  // Follow the tail while the reader is at the bottom.
  createEffect(
    on(count, (n) => {
      if (follow() && stickToBottom && n > 0) virtualizer.scrollToIndex(n - 1, { align: "end" });
    }),
  );

  function onScroll() {
    if (!scrollEl) return;
    stickToBottom = scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight < LINE_HEIGHT * 2;
  }

  return (
    <div class="flex h-full flex-col">
      <div class="flex flex-wrap items-center gap-2 border-b border-line px-2 py-1.5 text-[12px]">
        <Show when={props.containers.length > 1}>
          <select aria-label="Container" class="rounded border border-line bg-canvas px-1 py-0.5" onChange={(e) => setContainer(e.currentTarget.value)}>
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
        <select
          aria-label="History"
          class="rounded border border-line bg-canvas px-1 py-0.5"
          onChange={(e) => setTail(TAIL_CHOICES[Number(e.currentTarget.value)]!.value)}
        >
          <For each={TAIL_CHOICES}>
            {(choice, i) => (
              <option value={i()} selected={choice.value === tail()}>
                {choice.label}
              </option>
            )}
          </For>
        </select>
        <Toggle label="Timestamps" value={timestamps()} onChange={setTimestamps} />
        <Toggle label="Previous" value={previous()} onChange={setPrevious} />
        <Toggle label="Auto-scroll" value={follow()} onChange={setFollow} />
        <input
          type="search"
          aria-label="Filter logs"
          placeholder="Filter lines"
          class="min-w-24 flex-1 rounded border border-line bg-canvas px-2 py-0.5"
          value={filter()}
          onInput={(e) => setFilter(e.currentTarget.value)}
        />
        <span class="tabular-nums text-muted">
          {count().toLocaleString()}
          <Show when={filter()}> / {buffer.totalCount.toLocaleString()}</Show> lines
          <Show when={buffer.dropped > 0}> · {buffer.dropped.toLocaleString()} older dropped</Show>
          <Show when={status() === "ended"}> · stream ended</Show>
        </span>
      </div>
      <Show when={error()}>
        {(e) => (
          <div class="p-2">
            <ErrorNotice compact error={e()} title="Log stream failed" />
          </div>
        )}
      </Show>
      <div ref={scrollEl} class="selectable min-h-0 flex-1 overflow-auto font-mono text-[12px]" onScroll={onScroll}>
        <div style={{ height: `${virtualizer.getTotalSize()}px`, position: "relative", "min-width": "100%", width: "max-content" }}>
          <For each={virtualizer.getVirtualItems()}>
            {(item) => (
              <div
                class="absolute top-0 left-0 whitespace-pre px-2"
                style={{ transform: `translateY(${item.start}px)`, height: `${LINE_HEIGHT}px`, "line-height": `${LINE_HEIGHT}px` }}
              >
                {(void revision(), buffer.line(item.index))}
              </div>
            )}
          </For>
        </div>
      </div>
    </div>
  );
}

function Toggle(props: { label: string; value: boolean; onChange: (v: boolean) => void }) {
  return (
    <label class="flex items-center gap-1">
      <input type="checkbox" checked={props.value} onChange={(e) => props.onChange(e.currentTarget.checked)} />
      {props.label}
    </label>
  );
}
