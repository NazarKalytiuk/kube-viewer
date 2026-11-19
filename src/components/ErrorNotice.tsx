import { Show } from "solid-js";
import { errorHint } from "../lib/format";
import type { AppError } from "../lib/types";

export function ErrorNotice(props: { error: AppError; title?: string; onRetry?: () => void; compact?: boolean }) {
  return (
    <div
      role="alert"
      class="rounded-md border border-error/40 bg-error-bg text-fg"
      classList={{ "p-4": !props.compact, "px-3 py-2": props.compact }}
    >
      <div class="flex items-start gap-3">
        <div class="min-w-0 flex-1">
          <div class="font-semibold text-error">{props.title ?? "Something went wrong"}</div>
          <div class="selectable mt-1 break-words font-mono text-[12px]">{props.error.message}</div>
          <Show when={errorHint(props.error)}>{(hint) => <div class="mt-2 text-muted">{hint()}</div>}</Show>
        </div>
        <Show when={props.onRetry}>
          {(retry) => (
            <button
              class="shrink-0 rounded border border-line bg-canvas px-3 py-1 hover:bg-hover"
              onClick={() => retry()()}
            >
              Retry
            </button>
          )}
        </Show>
      </div>
    </div>
  );
}
