// Bounded log line buffer with an incrementally maintained filter.

export const MAX_LOG_LINES = 200_000;

export class LogBuffer {
  private lines: string[] = [];
  private matches: number[] | null = null;
  private needle = "";
  /** Lines discarded from the front to stay within the bound. */
  dropped = 0;

  constructor(private readonly maxLines = MAX_LOG_LINES) {}

  get totalCount(): number {
    return this.lines.length;
  }

  /** Lines currently shown (after filtering). */
  get visibleCount(): number {
    return this.matches ? this.matches.length : this.lines.length;
  }

  line(index: number): string {
    const i = this.matches ? this.matches[index] : index;
    return i === undefined ? "" : (this.lines[i] ?? "");
  }

  append(batch: string[]) {
    const base = this.lines.length;
    for (let i = 0; i < batch.length; i++) {
      const line = batch[i]!;
      this.lines.push(line);
      if (this.matches && line.toLowerCase().includes(this.needle)) this.matches.push(base + i);
    }
    if (this.lines.length > this.maxLines) {
      // Drop in chunks (10% headroom) so the splice cost is amortized.
      const drop = this.lines.length - this.maxLines + Math.floor(this.maxLines / 10);
      this.lines.splice(0, drop);
      this.dropped += drop;
      if (this.matches) {
        this.matches = this.matches.filter((i) => i >= drop).map((i) => i - drop);
      }
    }
  }

  setFilter(filter: string) {
    this.needle = filter.trim().toLowerCase();
    if (!this.needle) {
      this.matches = null;
      return;
    }
    const matches: number[] = [];
    for (let i = 0; i < this.lines.length; i++) {
      if (this.lines[i]!.toLowerCase().includes(this.needle)) matches.push(i);
    }
    this.matches = matches;
  }

  clear() {
    this.lines = [];
    this.dropped = 0;
    this.matches = this.needle ? [] : null;
  }
}
