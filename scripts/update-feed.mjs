#!/usr/bin/env node
// Writes the updater feed (latest.json) for a GitHub release.
//
// Archive URLs use the public release download path, which is stable once the
// release is published and, unlike api.github.com asset URLs, is not subject to
// the anonymous API rate limit (60 requests/hour per IP, shared behind NAT).
//
//   node scripts/update-feed.mjs --repo owner/name --tag v1.2.3 --asset <archive asset name> \
//     --signature <path to .sig> --notes <path to notes> > latest.json

import { readFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { fileURLToPath } from "node:url";

/** The universal archive serves both architectures. */
const PLATFORMS = ["darwin-aarch64", "darwin-x86_64"];

export function buildFeed({ repo, tag, asset, signature, notes, pubDate }) {
  if (!/^[\w.-]+\/[\w.-]+$/.test(repo)) throw new Error(`invalid repository "${repo}"`);
  if (!/^v\d+\.\d+\.\d+/.test(tag)) throw new Error(`invalid tag "${tag}"`);
  if (!asset.endsWith(".app.tar.gz")) throw new Error(`"${asset}" is not an updater archive`);
  const sig = signature.trim();
  if (!sig) throw new Error("empty signature");
  const url = `https://github.com/${repo}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(asset)}`;
  return {
    version: tag.slice(1),
    notes: notes.trim(),
    pub_date: pubDate.toISOString().replace(/\.\d{3}Z$/, "Z"),
    platforms: Object.fromEntries(PLATFORMS.map((p) => [p, { signature: sig, url }])),
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const { values } = parseArgs({
    options: {
      repo: { type: "string" },
      tag: { type: "string" },
      asset: { type: "string" },
      signature: { type: "string" },
      notes: { type: "string" },
    },
  });
  for (const k of ["repo", "tag", "asset", "signature", "notes"]) {
    if (!values[k]) {
      console.error(`update-feed: --${k} is required`);
      process.exit(2);
    }
  }
  const feed = buildFeed({
    repo: values.repo,
    tag: values.tag,
    asset: values.asset,
    signature: readFileSync(values.signature, "utf8"),
    notes: readFileSync(values.notes, "utf8"),
    pubDate: new Date(),
  });
  process.stdout.write(`${JSON.stringify(feed, null, 2)}\n`);
}
