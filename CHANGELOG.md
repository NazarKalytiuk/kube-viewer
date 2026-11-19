# Changelog

Notable changes to KubeViewer. Versions follow [Semantic Versioning](https://semver.org/).
Each release section becomes the GitHub Release notes and the "What’s new" text in the in-app updater.

## Unreleased

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
