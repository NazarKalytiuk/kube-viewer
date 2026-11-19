// Client-side cache of the items around the visible range of a Rust-side
// list (table rows, log lines).
//
// The full data set stays in Rust; this cache only ever holds the visible
// window plus overscan. Requests are coalesced: at most one is in flight and
// the latest wanted range/version wins, so fast scrolling or a burst of
// updates never queues up stale IPC calls.

export interface Window<T> {
  version: number;
  total: number;
  start: number;
  items: T[];
}

export type WindowFetcher<T> = (start: number, end: number) => Promise<Window<T>>;

export interface WindowCacheOptions {
  /** Extra rows fetched on each side of the visible range. */
  overscan?: number;
  /** Upper bound of cached rows (backend caps a request at 1000). */
  maxCached?: number;
}

export class WindowCache<T> {
  private rows = new Map<number, T>();
  private cachedVersion = -1;
  private wantedVersion = 0;
  private visible = { start: 0, end: 0 };
  private inflight = false;
  private rerun = false;
  private disposed = false;
  private readonly overscan: number;
  private readonly maxCached: number;

  constructor(
    private readonly fetch: WindowFetcher<T>,
    private readonly onChange: (total: number) => void,
    private readonly onError: (error: unknown) => void = () => {},
    options: WindowCacheOptions = {},
  ) {
    this.overscan = options.overscan ?? 40;
    this.maxCached = options.maxCached ?? 3000;
  }

  get(index: number): T | undefined {
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

  /** Drops everything and refetches (e.g. when the item formatting changed). */
  reset() {
    this.rows = new Map();
    this.cachedVersion = -1;
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
        let result: Window<T>;
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

  private apply(result: Window<T>) {
    if (result.version < this.cachedVersion) return;
    if (result.version !== this.cachedVersion) {
      // New data version: everything outside this response is stale.
      this.rows = new Map();
      this.cachedVersion = result.version;
    }
    this.wantedVersion = Math.max(this.wantedVersion, result.version);
    result.items.forEach((item, i) => this.rows.set(result.start + i, item));
    // Drop rows beyond the total (deletions) and far-away rows (memory bound).
    for (const index of this.rows.keys()) {
      if (index >= result.total) this.rows.delete(index);
    }
    if (this.rows.size > this.maxCached) {
      const center = (this.visible.start + this.visible.end) / 2;
      const keep = [...this.rows.keys()].sort((a, b) => Math.abs(a - center) - Math.abs(b - center)).slice(0, this.maxCached);
      const kept = new Map<number, T>();
      for (const index of keep) kept.set(index, this.rows.get(index)!);
      this.rows = kept;
    }
    this.onChange(result.total);
  }
}
