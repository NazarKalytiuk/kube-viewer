// Turns a raw log line into styled segments: ANSI SGR colors, log-level
// keywords and search highlights. Runs only for the ~100 visible lines.

export interface Segment {
  text: string;
  /** Tailwind/CSS classes. */
  cls: string;
  /** Inline color from ANSI (palette or 24-bit). */
  color?: string;
  background?: string;
}

interface Style {
  fg?: string;
  bg?: string;
  bold?: boolean;
  dim?: boolean;
  italic?: boolean;
  underline?: boolean;
}

// Standard + bright ANSI colors, readable on both light and dark backgrounds.
const PALETTE = [
  "#6b7280", "#dc2626", "#16a34a", "#ca8a04", "#2563eb", "#c026d3", "#0891b2", "#9ca3af",
  "#9ca3af", "#ef4444", "#22c55e", "#eab308", "#3b82f6", "#d946ef", "#06b6d4", "#e5e7eb",
];

function xterm256(n: number): string {
  if (n < 16) return PALETTE[n]!;
  if (n < 232) {
    const v = n - 16;
    const c = (x: number) => (x === 0 ? 0 : 55 + x * 40);
    return `rgb(${c(Math.floor(v / 36))},${c(Math.floor(v / 6) % 6)},${c(v % 6)})`;
  }
  const g = 8 + (n - 232) * 10;
  return `rgb(${g},${g},${g})`;
}

function applySgr(style: Style, params: number[]): Style {
  const s = { ...style };
  if (params.length === 0) params = [0];
  for (let i = 0; i < params.length; i++) {
    const p = params[i]!;
    if (p === 0) {
      for (const k of Object.keys(s) as (keyof Style)[]) delete s[k];
    } else if (p === 1) s.bold = true;
    else if (p === 2) s.dim = true;
    else if (p === 3) s.italic = true;
    else if (p === 4) s.underline = true;
    else if (p === 22) s.bold = s.dim = false;
    else if (p === 23) s.italic = false;
    else if (p === 24) s.underline = false;
    else if (p >= 30 && p <= 37) s.fg = PALETTE[p - 30];
    else if (p >= 90 && p <= 97) s.fg = PALETTE[p - 90 + 8];
    else if (p === 39) delete s.fg;
    else if (p >= 40 && p <= 47) s.bg = PALETTE[p - 40];
    else if (p >= 100 && p <= 107) s.bg = PALETTE[p - 100 + 8];
    else if (p === 49) delete s.bg;
    else if ((p === 38 || p === 48) && params[i + 1] === 5 && params[i + 2] !== undefined) {
      const color = xterm256(params[i + 2]!);
      if (p === 38) s.fg = color;
      else s.bg = color;
      i += 2;
    } else if ((p === 38 || p === 48) && params[i + 1] === 2 && params[i + 4] !== undefined) {
      const color = `rgb(${params[i + 2]},${params[i + 3]},${params[i + 4]})`;
      if (p === 38) s.fg = color;
      else s.bg = color;
      i += 4;
    }
  }
  return s;
}

// eslint-disable-next-line no-control-regex
const ESCAPE = /\x1b\[([0-9;]*)([A-Za-z])/g;

interface Span {
  start: number;
  end: number;
  style: Style;
}

/** Strips ANSI escapes, returning plain text and its style spans. */
export function parseAnsi(raw: string): { text: string; spans: Span[] } {
  if (!raw.includes("\x1b")) return { text: raw, spans: [{ start: 0, end: raw.length, style: {} }] };
  let text = "";
  const spans: Span[] = [];
  let style: Style = {};
  let last = 0;
  for (const m of raw.matchAll(ESCAPE)) {
    const chunk = raw.slice(last, m.index);
    if (chunk) {
      spans.push({ start: text.length, end: text.length + chunk.length, style });
      text += chunk;
    }
    if (m[2] === "m") {
      style = applySgr(style, m[1] ? m[1].split(";").map((x) => Number(x) || 0) : []);
    }
    last = m.index + m[0].length;
  }
  const rest = raw.slice(last);
  if (rest) {
    spans.push({ start: text.length, end: text.length + rest.length, style });
    text += rest;
  }
  return { text, spans };
}

export type Level = "error" | "warn" | null;

const LEVEL = /\b(FATAL|PANIC|CRITICAL|ERROR|ERR|WARNING|WARN)\b|"level"\s*:\s*"(fatal|panic|error|warn|warning)"|\blevel=(fatal|panic|error|warn|warning)\b/i;

/** Range of the log-level token, if the line has one. */
export function findLevel(text: string): { level: Level; start: number; end: number } | null {
  const m = LEVEL.exec(text);
  if (!m) return null;
  const word = (m[1] ?? m[2] ?? m[3] ?? "").toLowerCase();
  const level: Level = word.startsWith("warn") ? "warn" : "error";
  return { level, start: m.index, end: m.index + m[0].length };
}

/** Builds a JS RegExp matching the backend's search semantics. */
export function searchRegExp(query: { text: string; regex: boolean; caseSensitive: boolean } | null): RegExp | null {
  if (!query || !query.text) return null;
  const source = query.regex ? query.text : query.text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  try {
    return new RegExp(source, query.caseSensitive ? "g" : "gi");
  } catch {
    return null;
  }
}

const LEVEL_CLASS = { error: "text-error font-semibold", warn: "text-warn font-semibold" } as const;

/** Splits a line into segments with ANSI colors, level and search marks. */
export function segmentLine(raw: string, search: RegExp | null): Segment[] {
  const { text, spans } = parseAnsi(raw);
  // Boundaries: ANSI spans, level token, search matches.
  const marks: { start: number; end: number; cls: string }[] = [];
  const level = findLevel(text);
  if (level?.level) marks.push({ start: level.start, end: level.end, cls: LEVEL_CLASS[level.level] });
  if (search) {
    search.lastIndex = 0;
    for (const m of text.matchAll(search)) {
      if (m[0].length === 0) break;
      marks.push({ start: m.index, end: m.index + m[0].length, cls: "kv-mark" });
    }
  }
  const cuts = new Set<number>([0, text.length]);
  for (const s of spans) cuts.add(s.start).add(s.end);
  for (const m of marks) cuts.add(m.start).add(m.end);
  const points = [...cuts].sort((a, b) => a - b);

  const out: Segment[] = [];
  for (let i = 0; i < points.length - 1; i++) {
    const start = points[i]!;
    const end = points[i + 1]!;
    if (start === end) continue;
    const span = spans.find((s) => s.start <= start && end <= s.end);
    const style = span?.style ?? {};
    const cls = [
      style.bold ? "font-semibold" : "",
      style.dim ? "opacity-60" : "",
      style.italic ? "italic" : "",
      style.underline ? "underline" : "",
      ...marks.filter((m) => m.start <= start && end <= m.end).map((m) => m.cls),
    ]
      .filter(Boolean)
      .join(" ");
    const seg: Segment = { text: text.slice(start, end), cls };
    if (style.fg) seg.color = style.fg;
    if (style.bg) seg.background = style.bg;
    const prev = out[out.length - 1];
    if (prev && prev.cls === seg.cls && prev.color === seg.color && prev.background === seg.background) {
      prev.text += seg.text;
    } else {
      out.push(seg);
    }
  }
  return out;
}
