import { describe, expect, it } from "vitest";
import { LogBuffer } from "./logBuffer";

describe("LogBuffer", () => {
  it("appends and reads lines", () => {
    const b = new LogBuffer();
    b.append(["a", "b"]);
    b.append(["c"]);
    expect(b.visibleCount).toBe(3);
    expect([0, 1, 2].map((i) => b.line(i))).toEqual(["a", "b", "c"]);
    expect(b.line(9)).toBe("");
  });

  it("filters case-insensitively, including lines appended later", () => {
    const b = new LogBuffer();
    b.append(["INFO start", "ERROR boom", "info ok"]);
    b.setFilter(" error ");
    expect(b.visibleCount).toBe(1);
    b.append(["error again", "debug"]);
    expect([0, 1].map((i) => b.line(i))).toEqual(["ERROR boom", "error again"]);
    expect(b.totalCount).toBe(5);
    b.setFilter("");
    expect(b.visibleCount).toBe(5);
  });

  it("drops the oldest lines beyond the bound and keeps filter indices valid", () => {
    const b = new LogBuffer(100);
    b.setFilter("x");
    b.append(Array.from({ length: 150 }, (_, i) => (i % 10 === 0 ? `x${i}` : `${i}`)));
    // 150 > 100 → drop 150 - 100 + 10 = 60 oldest.
    expect(b.dropped).toBe(60);
    expect(b.totalCount).toBe(90);
    expect(b.line(0)).toBe("x60");
    expect(b.visibleCount).toBe(9);
  });

  it("clear keeps the active filter", () => {
    const b = new LogBuffer();
    b.setFilter("x");
    b.append(["x1"]);
    b.clear();
    b.append(["x2", "y"]);
    expect(b.visibleCount).toBe(1);
    expect(b.dropped).toBe(0);
  });
});
