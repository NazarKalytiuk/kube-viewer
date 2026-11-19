// Client-side cache of the rows around the visible range of a Rust-side view.
//
// The full data set stays in Rust; this cache only ever holds the visible
// window plus overscan. Requests are coalesced: at most one is in flight and
// the latest wanted range/version wins, so fast scrolling or a burst of watch
// updates never queues up stale IPC calls.

import type { Row, RowsWindow } from "./types";

export type RowFetcher = (start: number, end: number) => Promise<RowsWindow>;

export interface RowWindowOptions {
  /** Extra rows fetched on each side of the visible range. */
  overscan?: number;
  /** Upper bound of cached rows (backend caps a request at 1000). */
  maxCached?: number;
}

export class RowWindow {
  private rows = new Map<number, Row>();
  private cachedVersion = -1;
  private wantedVersion = 0;
  private visible = { start: 0, end: 0 };
  private inflight = false;
  private rerun = false;
  private disposed = false;
  private readonly overscan: number;
  private readonly maxCached: number;

  constructor(
    private readonly fetch: RowFetcher,
    private readonly onChange: (total: number) => void,
    private readonly onError: (error: unknown) => void = () => {},
    options: RowWindowOptions = {},
  ) {
    this.overscan = options.overscan ?? 40;
    this.maxCached = options.maxCached ?? 3000;
  }

  get(index: number): Row | undefined {
    return this.rows.get(index);
  }

  /** Called by the virtualizer whenever the visible range changes. */
  setVisible(start: number, end: number) {
    this.visible = { start, end };
    if (this.needsFetch()) void this.run();
  }

  /** Called when the backend reports a new data version. */
  invalidate(version: number) {
    if (version <= this.cachedVersion && version <= this.wantedVersion) return;
    this.wantedVersion = Math.max(this.wantedVersion, version);
    void this.run();
  }

  dispose() {
    this.disposed = true;
    this.rows.clear();
  }

  private needsFetch(): boolean {
    if (this.cachedVersion < this.wantedVersion) return true;
    for (let i = this.visible.start; i < this.visible.end; i++) {
      if (!this.rows.has(i)) return true;
    }
    return false;
  }

  private async run() {
    if (this.inflight) {
      this.rerun = true;
      return;
    }
    this.inflight = true;
    try {
      do {
        this.rerun = false;
        const start = Math.max(0, this.visible.start - this.overscan);
        const end = Math.max(start, this.visible.end + this.overscan);
        let result: RowsWindow;
        try {
          result = await this.fetch(start, Math.min(end, start + 1000));
        } catch (error) {
          if (!this.disposed) this.onError(error);
          return;
        }
        if (this.disposed) return;
        this.apply(result);
      } while (this.rerun || this.needsFetch());
    } finally {
      this.inflight = false;
    }
  }

  private apply(result: RowsWindow) {
    if (result.version < this.cachedVersion) return;
    if (result.version !== this.cachedVersion) {
      // New data version: everything outside this response is stale.
      this.rows = new Map();
      this.cachedVersion = result.version;
    }
    this.wantedVersion = Math.max(this.wantedVersion, result.version);
    result.rows.forEach((row, i) => this.rows.set(result.start + i, row));
    // Drop rows beyond the total (deletions) and far-away rows (memory bound).
    for (const index of this.rows.keys()) {
      if (index >= result.total) this.rows.delete(index);
    }
    if (this.rows.size > this.maxCached) {
      const center = (this.visible.start + this.visible.end) / 2;
      const keep = [...this.rows.keys()].sort((a, b) => Math.abs(a - center) - Math.abs(b - center)).slice(0, this.maxCached);
      const kept = new Map<number, Row>();
      for (const index of keep) kept.set(index, this.rows.get(index)!);
      this.rows = kept;
    }
    this.onChange(result.total);
  }
}
