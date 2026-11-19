import { describe, expect, it } from "vitest";
import { authLabel, cellText, formatAge } from "./format";

describe("formatAge", () => {
  const age = (seconds: number) => formatAge(0, seconds);

  it("matches kubectl's human durations", () => {
    expect(age(5)).toBe("5s");
    expect(age(119)).toBe("119s");
    expect(age(125)).toBe("2m5s");
    expect(age(600)).toBe("10m");
    expect(age(3 * 3600 + 15 * 60)).toBe("3h15m");
    expect(age(30 * 3600)).toBe("30h");
    expect(age(3 * 86400 + 4 * 3600)).toBe("3d4h");
    expect(age(400 * 86400)).toBe("400d");
    expect(age(3 * 365 * 86400 + 12 * 86400)).toBe("3y12d");
    expect(age(9 * 365 * 86400)).toBe("9y");
  });

  it("never shows negative ages (clock skew)", () => {
    expect(formatAge(100, 50)).toBe("0s");
  });
});

describe("cellText", () => {
  it("renders each cell kind", () => {
    expect(cellText(null, "text", 0)).toBe("");
    expect(cellText("web", "text", 0)).toBe("web");
    expect(cellText(3, "number", 0)).toBe("3");
    expect(cellText(100, "age", 160)).toBe("60s");
    expect(cellText({ t: "Running", tone: "ok" }, "status", 0)).toBe("Running");
  });
});

describe("authLabel", () => {
  it("shows the exec plugin binary name", () => {
    expect(authLabel({ type: "exec", command: "/usr/local/bin/aws" })).toBe("exec: aws");
    expect(authLabel({ type: "client_certificate" })).toBe("client certificate");
  });
});
