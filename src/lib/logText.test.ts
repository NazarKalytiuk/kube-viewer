import { describe, expect, it } from "vitest";
import { findLevel, parseAnsi, searchRegExp, segmentLine } from "./logText";

describe("parseAnsi", () => {
  it("strips escapes and keeps styles per span", () => {
    const { text, spans } = parseAnsi("\x1b[31mred\x1b[0m plain \x1b[1;38;5;46mbold-green\x1b[m");
    expect(text).toBe("red plain bold-green");
    expect(spans[0]).toMatchObject({ start: 0, end: 3, style: { fg: "#dc2626" } });
    expect(spans[1]!.style).toEqual({});
    expect(spans[2]!.style).toMatchObject({ bold: true, fg: "rgb(0,255,0)" });
  });

  it("ignores non-color escape sequences", () => {
    expect(parseAnsi("a\x1b[2Kb").text).toBe("ab");
  });

  it("fast path for plain text", () => {
    expect(parseAnsi("plain").spans).toHaveLength(1);
  });
});

describe("findLevel", () => {
  it("detects common level formats", () => {
    expect(findLevel("2026 ERROR db down")?.level).toBe("error");
    expect(findLevel('{"level":"warn","msg":"x"}')?.level).toBe("warn");
    expect(findLevel("level=error msg=x")?.level).toBe("error");
    expect(findLevel("everything fine")).toBeNull();
    expect(findLevel("terrorist")).toBeNull();
  });
});

describe("segmentLine", () => {
  it("marks search matches and levels across ANSI spans", () => {
    const segs = segmentLine("\x1b[32mINFO\x1b[0m request failed: ERROR timeout", searchRegExp({ text: "fail", regex: false, caseSensitive: false }));
    expect(segs.map((s) => s.text).join("")).toBe("INFO request failed: ERROR timeout");
    expect(segs.find((s) => s.text === "fail")?.cls).toContain("kv-mark");
    expect(segs.find((s) => s.text === "ERROR")?.cls).toContain("text-error");
    expect(segs.find((s) => s.text === "INFO")?.color).toBe("#16a34a");
  });

  it("escapes literal search text and tolerates invalid regex", () => {
    expect(searchRegExp({ text: "a.b", regex: false, caseSensitive: true })!.test("axb")).toBe(false);
    expect(searchRegExp({ text: "(", regex: true, caseSensitive: false })).toBeNull();
  });
});
