# Changelog

Notable changes to KubeViewer. Versions follow [Semantic Versioning](https://semver.org/).
Each release section becomes the GitHub Release notes and the "What’s new" text in the in-app updater.

## Unreleased

## 0.1.2 — 2026-11-28

- Fixed: logs no longer flicker while opening or when you change how much history is loaded. Lines no longer flash empty while the history streams in or new lines arrive.
- Fixed: filtering or sorting a table no longer briefly shows the previous rows before the new results.

## 0.1.1 — 2026-11-20

- Fixed: switching log history (for example from “Last 1,000 lines” to “Last 24 hours” or “Everything” and back) no longer freezes the logs view or keeps showing the previous lines.
- “All containers” logs now appear progressively while the history loads, and stay within the memory budget even with “Everything”.
- Searching, filtering and scrolling large logs and tables no longer block the window while data is loading.

## 0.1.0 — 2026-11-19

First public release.

- Browse 13 resource kinds across contexts and namespaces with live updates. Tables stay fast at 100k+ rows: data stays in Rust, and only the visible rows reach the UI.
- Details, conditions, events and syntax-highlighted YAML. Secret values stay hidden until you reveal them.
- Pod logs:
  - follow, history presets and previous instance;
  - all containers merged by time;
  - time shown as local, UTC or hidden;
  - find and filter with regex;
  - copy any range of lines.
- Download the full log from the cluster or save the loaded lines.
- In-app updates from GitHub Releases, verified with a signing key.
