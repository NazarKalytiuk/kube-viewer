import { describe, expect, it } from "vitest";
import { buildFeed } from "./update-feed.mjs";

const base = {
  repo: "NazarKalytiuk/kube-viewer",
  tag: "v0.2.0",
  asset: "KubeViewer_0.2.0_universal.app.tar.gz",
  signature: "c2lnbmF0dXJl\n",
  notes: "- Faster logs\n",
  pubDate: new Date("2026-12-01T10:20:30.456Z"),
};

describe("buildFeed", () => {
  it("points both macOS architectures at the universal archive's public URL", () => {
    const url = "https://github.com/NazarKalytiuk/kube-viewer/releases/download/v0.2.0/KubeViewer_0.2.0_universal.app.tar.gz";
    expect(buildFeed(base)).toEqual({
      version: "0.2.0",
      notes: "- Faster logs",
      pub_date: "2026-12-01T10:20:30Z",
      platforms: {
        "darwin-aarch64": { signature: "c2lnbmF0dXJl", url },
        "darwin-x86_64": { signature: "c2lnbmF0dXJl", url },
      },
    });
  });

  it("rejects inputs that would produce a broken feed", () => {
    expect(() => buildFeed({ ...base, asset: "KubeViewer_0.2.0_universal.dmg" })).toThrow(/updater archive/);
    expect(() => buildFeed({ ...base, signature: " \n" })).toThrow(/empty signature/);
    expect(() => buildFeed({ ...base, tag: "0.2.0" })).toThrow(/invalid tag/);
  });
});
