import { describe, expect, it } from "vitest";
import { WindowCache, type Window } from "./windowCache";
import type { Row } from "./types";

function makeRow(i: number, version: number): Row {
  return { uid: `u${i}`, name: `row-${i}-v${version}`, namespace: null, cells: [`row-${i}`] };
}

/** Fake backend with `total` rows; resolves requests manually. */
function backend(total: number) {
  let version = 1;
  const calls: { start: number; end: number; resolve: () => void }[] = [];
  const fetch = (start: number, end: number) =>
    new Promise<Window<Row>>((resolve) => {
      calls.push({
        start,
        end,
        resolve: () => {
          const rows: Row[] = [];
          for (let i = start; i < Math.min(end, total); i++) rows.push(makeRow(i, version));
          resolve({ version, total, start, items: rows });
        },
      });
    });
  return {
    fetch,
    calls,
    bump() {
      version += 1;
      return version;
    },
  };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("WindowCache", () => {
  it("fetches the visible range plus overscan", async () => {
    const b = backend(10_000);
    const w = new WindowCache<Row>(b.fetch, () => {}, undefined, { overscan: 10 });
    w.setVisible(100, 140);
    expect(b.calls).toHaveLength(1);
    expect(b.calls[0]).toMatchObject({ start: 90, end: 150 });
    b.calls[0]!.resolve();
    await flush();
    expect(w.get(100)?.name).toBe("row-100-v1");
    expect(w.get(149)).toBeDefined();
    expect(w.get(150)).toBeUndefined();
  });

  it("coalesces requests while one is in flight and fetches the latest range", async () => {
    const b = backend(10_000);
    const w = new WindowCache<Row>(b.fetch, () => {}, undefined, { overscan: 0 });
    w.setVisible(0, 50);
    w.setVisible(500, 550);
    w.setVisible(900, 950);
    expect(b.calls).toHaveLength(1);
    b.calls[0]!.resolve();
    await flush();
    expect(b.calls).toHaveLength(2);
    expect(b.calls[1]).toMatchObject({ start: 900, end: 950 });
  });

  it("refetches on a new version and drops stale rows", async () => {
    const b = backend(1000);
    const changes: number[] = [];
    const w = new WindowCache<Row>(b.fetch, (t) => changes.push(t), undefined, { overscan: 0 });
    w.setVisible(0, 20);
    b.calls[0]!.resolve();
    await flush();
    expect(w.get(0)?.name).toBe("row-0-v1");

    w.invalidate(b.bump());
    expect(b.calls).toHaveLength(2);
    // Old rows stay visible until the new version arrives (no flicker).
    expect(w.get(0)?.name).toBe("row-0-v1");
    b.calls[1]!.resolve();
    await flush();
    expect(w.get(0)?.name).toBe("row-0-v2");
    expect(changes).toEqual([1000, 1000]);
  });

  it("does not refetch when everything visible is cached", async () => {
    const b = backend(1000);
    const w = new WindowCache<Row>(b.fetch, () => {}, undefined, { overscan: 20 });
    w.setVisible(100, 120);
    b.calls[0]!.resolve();
    await flush();
    w.setVisible(105, 125);
    expect(b.calls).toHaveLength(1);
    w.invalidate(1); // same version
    expect(b.calls).toHaveLength(1);
  });

  it("reset drops cached items and refetches the same version", async () => {
    const b = backend(100);
    const w = new WindowCache<Row>(b.fetch, () => {}, undefined, { overscan: 0 });
    w.setVisible(0, 10);
    b.calls[0]!.resolve();
    await flush();
    w.reset();
    expect(w.get(0)).toBeUndefined();
    expect(b.calls).toHaveLength(2);
    b.calls[1]!.resolve();
    await flush();
    expect(w.get(0)?.name).toBe("row-0-v1");
  });

  it("does not fetch without a source and ignores versions meanwhile", async () => {
    const b = backend(100);
    const w = new WindowCache<Row>(null, () => {}, undefined, { overscan: 0 });
    w.setVisible(0, 10);
    w.invalidate(42);
    w.reset();
    expect(b.calls).toHaveLength(0);
    w.setSource(b.fetch);
    expect(b.calls).toHaveLength(1);
    b.calls[0]!.resolve();
    await flush();
    expect(w.get(0)?.name).toBe("row-0-v1");
    expect(b.calls).toHaveLength(1);
  });

  it("forgets the old source's versions when switching to a newer, smaller one", async () => {
    // e.g. "Everything" (version 5000) replaced by "Last 100 lines" (version 1).
    const old = backend(10_000);
    for (let i = 0; i < 4999; i++) old.bump();
    const w = new WindowCache<Row>(old.fetch, () => {}, undefined, { overscan: 0 });
    w.setVisible(0, 10);
    old.calls[0]!.resolve();
    await flush();
    expect(w.get(0)?.name).toBe("row-0-v5000");

    const next = backend(100);
    w.setSource(next.fetch);
    next.calls[0]!.resolve();
    await flush();
    expect(w.get(0)?.name).toBe("row-0-v1");
    await flush();
    // Settled: no fetch loop chasing the old source's version.
    expect(next.calls).toHaveLength(1);
    w.invalidate(next.bump());
    expect(next.calls).toHaveLength(2);
  });

  it("drops a response from the previous source that arrives after the switch", async () => {
    const old = backend(1000);
    for (let i = 0; i < 99; i++) old.bump();
    const w = new WindowCache<Row>(old.fetch, () => {}, undefined, { overscan: 0 });
    w.setVisible(0, 10);
    const next = backend(1000);
    w.setSource(next.fetch);
    // The old request (version 100) was still in flight.
    old.calls[0]!.resolve();
    await flush();
    expect(w.get(0)).toBeUndefined();
    expect(next.calls).toHaveLength(1);
    next.calls[0]!.resolve();
    await flush();
    expect(w.get(0)?.name).toBe("row-0-v1");
    // A later update of the new source is not mistaken for stale.
    w.invalidate(next.bump());
    next.calls[1]!.resolve();
    await flush();
    expect(w.get(0)?.name).toBe("row-0-v2");
  });

  it("ignores errors of a source it already left", async () => {
    const errors: unknown[] = [];
    let reject: (e: unknown) => void = () => {};
    const closed = () => new Promise<Window<Row>>((_, r) => (reject = r));
    const w = new WindowCache<Row>(closed, () => {}, (e) => errors.push(e), { overscan: 0 });
    w.setVisible(0, 10);
    const next = backend(100);
    w.setSource(next.fetch);
    reject({ code: "invalid_request", message: "unknown log view 1" });
    await flush();
    expect(errors).toEqual([]);
    next.calls[0]!.resolve();
    await flush();
    expect(w.get(0)?.name).toBe("row-0-v1");
  });

  it("reset discards a response fetched before it", async () => {
    let format = "a";
    const calls: (() => void)[] = [];
    const fetch = (start: number, end: number) =>
      new Promise<Window<Row>>((resolve) => {
        const f = format;
        calls.push(() => {
          const items: Row[] = [];
          for (let i = start; i < end; i++) items.push({ ...makeRow(i, 1), name: `${f}${i}` });
          resolve({ version: 1, total: 100, start, items });
        });
      });
    const w = new WindowCache<Row>(fetch, () => {}, undefined, { overscan: 0 });
    w.setVisible(0, 10);
    format = "b";
    w.reset();
    calls[0]!();
    await flush();
    expect(w.get(0)).toBeUndefined();
    calls[1]!();
    await flush();
    expect(w.get(0)?.name).toBe("b0");
  });

  it("reports fetch errors and stops", async () => {
    const errors: unknown[] = [];
    const w = new WindowCache<Row>(() => Promise.reject({ code: "invalid_request", message: "unknown view" }), () => {}, (e) => errors.push(e));
    w.setVisible(0, 10);
    await flush();
    expect(errors).toEqual([{ code: "invalid_request", message: "unknown view" }]);
  });
});

/** Fake log session: rows are appended, every append bumps the version. */
function growing(total: number) {
  let version = 1;
  const calls: ({ start: number; end: number } | { tail: number })[] = [];
  const respond = (start: number, end: number) =>
    Promise.resolve<Window<Row>>({ version, total, start, items: Array.from({ length: Math.max(0, end - start) }, (_, i) => makeRow(start + i, version)) });
  return {
    calls,
    range: (start: number, end: number) => {
      calls.push({ start, end });
      return respond(start, Math.min(end, total));
    },
    tail: (count: number) => {
      calls.push({ tail: count });
      return respond(Math.max(0, total - count), total);
    },
    append(n: number) {
      total += n;
      version += 1;
      return version;
    },
  };
}

describe("WindowCache following the end", () => {
  it("reports a grown total only together with the items at its end", async () => {
    const b = growing(100);
    const totals: number[] = [];
    const w = new WindowCache<Row>(null, (t) => totals.push(t), undefined, { overscan: 5 });
    w.setFollow(true);
    w.setSource(b.range, b.tail);
    await flush();
    expect(b.calls).toEqual([{ tail: 10 }]);
    expect(totals).toEqual([100]);

    w.setVisible(90, 100);
    w.invalidate(b.append(500));
    await flush();
    expect(b.calls[1]).toEqual({ tail: 20 });
    expect(totals).toEqual([100, 600]);
    for (let i = 590; i < 600; i++) expect(w.get(i)?.name).toBe(`row-${i}-v2`);
  });

  it("fetches nothing for a visible range left behind by the grown end", async () => {
    const b = growing(100);
    const w = new WindowCache<Row>(null, () => {}, undefined, { overscan: 5 });
    w.setFollow(true);
    w.setSource(b.range, b.tail);
    w.setVisible(90, 100);
    w.invalidate(b.append(500));
    await flush();
    const calls = b.calls.length;
    // The view still shows the old end until it scrolls to the new one.
    w.setVisible(90, 100);
    w.setVisible(590, 600);
    await flush();
    expect(b.calls).toHaveLength(calls);
  });

  it("fetches visible ranges again once it stops following", async () => {
    const b = growing(1000);
    const w = new WindowCache<Row>(null, () => {}, undefined, { overscan: 0 });
    w.setFollow(true);
    w.setSource(b.range, b.tail);
    w.setVisible(990, 1000);
    await flush();
    w.setFollow(false);
    w.setVisible(100, 110);
    await flush();
    expect(b.calls.at(-1)).toEqual({ start: 100, end: 110 });
    expect(w.get(100)).toBeDefined();
  });

  it("fetches ranges when the source has no tail", async () => {
    const b = growing(100);
    const w = new WindowCache<Row>(b.range, () => {}, undefined, { overscan: 0 });
    w.setFollow(true);
    w.setVisible(0, 10);
    await flush();
    expect(b.calls.every((c) => "start" in c)).toBe(true);
    expect(b.calls.at(-1)).toEqual({ start: 0, end: 10 });
  });
});
