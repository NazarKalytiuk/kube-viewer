import { describe, expect, it } from "vitest";
import { RowWindow } from "./rowWindow";
import type { Row, RowsWindow } from "./types";

function makeRow(i: number, version: number): Row {
  return { uid: `u${i}`, name: `row-${i}-v${version}`, namespace: null, cells: [`row-${i}`] };
}

/** Fake backend with `total` rows; resolves requests manually. */
function backend(total: number) {
  let version = 1;
  const calls: { start: number; end: number; resolve: () => void }[] = [];
  const fetch = (start: number, end: number) =>
    new Promise<RowsWindow>((resolve) => {
      calls.push({
        start,
        end,
        resolve: () => {
          const rows: Row[] = [];
          for (let i = start; i < Math.min(end, total); i++) rows.push(makeRow(i, version));
          resolve({ version, total, start, rows });
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

describe("RowWindow", () => {
  it("fetches the visible range plus overscan", async () => {
    const b = backend(10_000);
    const w = new RowWindow(b.fetch, () => {}, undefined, { overscan: 10 });
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
    const w = new RowWindow(b.fetch, () => {}, undefined, { overscan: 0 });
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
    const w = new RowWindow(b.fetch, (t) => changes.push(t), undefined, { overscan: 0 });
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
    const w = new RowWindow(b.fetch, () => {}, undefined, { overscan: 20 });
    w.setVisible(100, 120);
    b.calls[0]!.resolve();
    await flush();
    w.setVisible(105, 125);
    expect(b.calls).toHaveLength(1);
    w.invalidate(1); // same version
    expect(b.calls).toHaveLength(1);
  });

  it("reports fetch errors and stops", async () => {
    const errors: unknown[] = [];
    const w = new RowWindow(() => Promise.reject({ code: "invalid_request", message: "unknown view" }), () => {}, (e) => errors.push(e));
    w.setVisible(0, 10);
    await flush();
    expect(errors).toEqual([{ code: "invalid_request", message: "unknown view" }]);
  });
});
