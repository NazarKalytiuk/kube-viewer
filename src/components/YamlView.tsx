// Read-only YAML viewer on CodeMirror 6 (virtualized rendering, search, folding).

import { yaml } from "@codemirror/lang-yaml";
import { HighlightStyle, foldGutter, foldKeymap, syntaxHighlighting } from "@codemirror/language";
import { highlightSelectionMatches, search, searchKeymap } from "@codemirror/search";
import { EditorState } from "@codemirror/state";
import { EditorView, keymap, lineNumbers } from "@codemirror/view";
import { tags } from "@lezer/highlight";
import { createEffect, on, onCleanup, onMount } from "solid-js";

const highlight = HighlightStyle.define([
  { tag: [tags.propertyName, tags.definition(tags.propertyName)], color: "var(--kv-syn-key)" },
  { tag: [tags.string, tags.special(tags.string)], color: "var(--kv-syn-string)" },
  { tag: [tags.number, tags.bool, tags.null], color: "var(--kv-syn-number)" },
  { tag: [tags.comment, tags.meta], color: "var(--kv-muted)" },
  { tag: [tags.punctuation, tags.separator], color: "var(--kv-muted)" },
]);

const theme = EditorView.theme({
  "&": { backgroundColor: "var(--kv-canvas)", color: "var(--kv-fg)" },
  ".cm-gutters": { backgroundColor: "var(--kv-panel)", color: "var(--kv-muted)", borderRight: "1px solid var(--kv-line)" },
  ".cm-activeLineGutter": { backgroundColor: "transparent" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": { backgroundColor: "var(--kv-selected)" },
  ".cm-searchMatch": { backgroundColor: "color-mix(in srgb, var(--kv-warn) 30%, transparent)" },
  ".cm-searchMatch-selected": { backgroundColor: "color-mix(in srgb, var(--kv-warn) 55%, transparent)" },
  ".cm-panels": { backgroundColor: "var(--kv-panel)", color: "var(--kv-fg)" },
  ".cm-panels input, .cm-panels button": { color: "var(--kv-fg)" },
});

export function YamlView(props: { text: string }) {
  let host: HTMLDivElement | undefined;
  let view: EditorView | undefined;

  onMount(() => {
    view = new EditorView({
      parent: host!,
      state: EditorState.create({
        doc: props.text,
        extensions: [
          EditorState.readOnly.of(true),
          EditorView.editable.of(false),
          // Keep focusable so Cmd+F / Cmd+A work.
          EditorView.contentAttributes.of({ tabindex: "0" }),
          lineNumbers(),
          foldGutter(),
          yaml(),
          syntaxHighlighting(highlight),
          search({ top: true }),
          highlightSelectionMatches(),
          keymap.of([...searchKeymap, ...foldKeymap]),
          theme,
        ],
      }),
    });
  });

  // Live updates replace the document but keep the reader's scroll position.
  createEffect(
    on(
      () => props.text,
      (text) => {
        if (!view || view.state.doc.toString() === text) return;
        view.dispatch({
          changes: { from: 0, to: view.state.doc.length, insert: text },
          effects: view.scrollSnapshot(),
        });
      },
      { defer: true },
    ),
  );

  onCleanup(() => view?.destroy());

  return <div ref={host} class="h-full" />;
}
