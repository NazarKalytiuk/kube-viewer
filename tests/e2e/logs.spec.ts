import { expect, test, type Page } from "@playwright/test";
import { installMockBackend } from "./mockBackend";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    window.localStorage.clear();
    // Full history, so the whole mocked log is loaded.
    window.localStorage.setItem("kubeviewer:logPrefs", JSON.stringify({ history: "all" }));
  });
  await page.addInitScript(installMockBackend, { podCount: 100, logLines: 50_000 });
  await page.goto("/");
  await page.locator('[role="row"][aria-rowindex]').first().click();
  await page.getByRole("tab", { name: "logs" }).click();
  await expect(page.getByText(/50,000 lines/)).toBeVisible();
});

const logLines = (page: Page) => page.locator("[data-line]");
/** Starts counting painted frames that show a line still waiting for its content. */
async function watchPlaceholders(page: Page) {
  await page.evaluate(() => {
    const w = window as any;
    w.__placeholderFrames = 0;
    const sample = () => {
      if (document.querySelector('[aria-label="Log lines"] [data-index]:not([data-line])')) w.__placeholderFrames++;
      requestAnimationFrame(sample);
    };
    requestAnimationFrame(sample);
  });
  return () => page.evaluate(() => (window as any).__placeholderFrames as number);
}
const lastCall = (page: Page, cmd: string) =>
  page.evaluate((c) => [...(window as any).__KV_MOCK__.calls].reverse().find((x: any) => x.cmd === c)?.args, cmd);

test("switching history replaces the lines and settles", async ({ page }) => {
  const fetches = () => page.evaluate(() => (window as any).__KV_MOCK__.calls.filter((c: any) => c.cmd === "get_log_lines").length);
  const settled = async () => {
    const before = await fetches();
    await page.waitForTimeout(300);
    expect(await fetches()).toBe(before);
  };
  const history = page.getByLabel("History");
  // Focus the viewer, not the select (arrow keys would change the choice).
  const toTop = async () => {
    await logLines(page).last().click();
    await page.keyboard.press("Meta+ArrowUp");
  };
  // Live lines push the current session's version above the next session's.
  await page.evaluate(() => (window as any).__KV_MOCK__.appendLogs(5));
  await expect(logLines(page).last()).toContainText("app live 4");

  await history.selectOption("tail100");
  await expect(page.getByText(/^100 lines/)).toBeVisible();
  await expect(logLines(page).last()).toContainText("app line 49999");
  await toTop();
  await expect(logLines(page).first()).toContainText("app line 49900");
  await settled();

  await history.selectOption("all");
  await expect(page.getByText(/50,000 lines/)).toBeVisible();
  await expect(logLines(page).last()).toContainText("app line 49999");
  await settled();

  await history.selectOption("tail1000");
  await expect(page.getByText(/^1,000 lines/)).toBeVisible();
  await toTop();
  await expect(logLines(page).first()).toContainText("app line 49000");
  await settled();
});

test("lines never blink while history streams in on open or history switch", async ({ page }) => {
  // Like the real backend: history arrives in batches, every call takes a while.
  await page.evaluate(() => {
    (window as any).__KV_MOCK__.setLatency(15);
    (window as any).__KV_MOCK__.streamBacklog(8, 40);
  });
  const placeholderFrames = await watchPlaceholders(page);

  // Reopen the viewer.
  await page.getByRole("tab", { name: "status" }).click();
  await page.getByRole("tab", { name: "logs" }).click();
  await expect(page.getByText(/50,000 lines · live/)).toBeVisible();
  await expect(logLines(page).last()).toContainText("app line 49999");

  await page.getByLabel("History").selectOption("tail10000");
  await expect(page.getByText(/^10,000 lines · live/)).toBeVisible();
  await expect(logLines(page).last()).toContainText("app line 49999");

  expect(await placeholderFrames()).toBe(0);
});

test("opens at the newest line with a bounded DOM", async ({ page }) => {
  await expect(logLines(page).last()).toContainText("app line 49999");
  expect(await logLines(page).count()).toBeLessThan(200);
});

test("switches time display without refetching the stream", async ({ page }) => {
  const opens = () => page.evaluate(() => (window as any).__KV_MOCK__.calls.filter((c: any) => c.cmd === "open_logs").length);
  const before = await opens();
  await expect(logLines(page).last()).toContainText("L 2026-11-19");

  await page.getByRole("radio", { name: "UTC" }).click();
  await expect(logLines(page).last()).toContainText(/2026-11-19T\d\d:\d\d:\d\d\.\d{3}Z/);
  await page.getByRole("radio", { name: "No time" }).click();
  await expect(logLines(page).last()).not.toContainText("2026-11-19");
  expect(await opens()).toBe(before);
  expect((await lastCall(page, "get_log_lines")).timeMode).toBe("none");
});

test("find jumps between matches and highlights them", async ({ page }) => {
  const search = page.getByLabel("Search logs");
  await search.fill("error");
  await expect(page.getByText("50 matches")).toBeVisible();
  await search.press("Enter");
  await expect(page.getByText("1 / 50")).toBeVisible();
  await expect(page.locator("[data-line].bg-selected")).toContainText("app line 0 ERROR");
  await search.press("Enter");
  await expect(page.getByText("2 / 50")).toBeVisible();
  await expect(page.locator("[data-line].bg-selected")).toContainText("app line 1000 ERROR");
  await search.press("Shift+Enter");
  await expect(page.getByText("1 / 50")).toBeVisible();
  await expect(page.locator(".kv-mark").first()).toBeVisible();
  // Jumping to a match pauses following without inventing "new" lines.
  await expect(page.getByRole("button", { name: "↓ Jump to end" })).toBeVisible();
});

test("filter shows only matching lines; invalid regex is reported", async ({ page }) => {
  await page.getByRole("radio", { name: "Filter" }).click();
  await page.getByLabel("Search logs").fill("warn");
  await expect(page.getByText(/50 of 50,000 lines/)).toBeVisible();
  await expect(logLines(page)).toHaveCount(50);

  await page.getByLabel(".*").check();
  await page.getByLabel("Search logs").fill("(");
  await expect(page.getByRole("alert")).toContainText("invalid regular expression");
});

test("merges all containers with tags", async ({ page }) => {
  await page.getByLabel("Container").selectOption({ label: "All containers" });
  await expect(page.getByText(/100,000 lines/)).toBeVisible();
  expect((await lastCall(page, "open_logs")).request.containers).toEqual(["app", "sidecar"]);
  await expect(logLines(page).last()).toContainText("sidecar");
});

test("auto-scroll pauses when scrolling up and counts new lines", async ({ page }) => {
  const scroller = page.getByLabel("Log lines");
  await scroller.hover();
  await page.mouse.wheel(0, -3000);
  await expect(page.getByRole("button", { name: "↓ Jump to end" })).toBeVisible();
  await page.evaluate(() => (window as any).__KV_MOCK__.appendLogs(7));
  const pill = page.getByRole("button", { name: "↓ 7 new lines" });
  await expect(pill).toBeVisible();
  await pill.click();
  await expect(logLines(page).last()).toContainText("app live 6");
});

test("downloads the full log in the chosen time format", async ({ page }) => {
  await page.getByRole("button", { name: "Download…" }).click();
  await page.getByLabel("Time format").selectOption("local");
  await page.getByRole("button", { name: "Save as…" }).click();
  await expect(page.getByText(/Saved 50,000 lines/)).toBeVisible();
  await expect(page.getByRole("button", { name: "Show in Finder" })).toBeVisible();
  const args = await lastCall(page, "download_logs");
  expect(args.request).toMatchObject({ containers: ["app"], timeMode: "local", previous: false, containerPrefix: false });
  expect(args.request.path).toMatch(/^\/Users\/test\/Downloads\/pod-000000_app_\d{8}-\d{6}\.log$/);
});

test("saves exactly the filtered lines", async ({ page }) => {
  await page.getByRole("radio", { name: "Filter" }).click();
  await page.getByLabel("Search logs").fill("error");
  await expect(page.getByText(/50 of 50,000 lines/)).toBeVisible();
  await page.getByRole("button", { name: "Download…" }).click();
  await page.getByRole("radio", { name: /Loaded lines \(50, filtered\)/ }).click();
  await page.getByRole("button", { name: "Save as…" }).click();
  await expect(page.getByText(/Saved 50 lines/)).toBeVisible();
});

test("expands the panel to full width and back, keeping the table alive", async ({ page }) => {
  await page.getByRole("button", { name: "Expand panel" }).click();
  await expect(page.getByRole("grid")).toBeHidden();
  const box = await page.getByLabel("Log lines").boundingBox();
  expect(box!.width).toBeGreaterThan(1000);
  await page.getByRole("button", { name: "Restore panel" }).click();
  await expect(page.getByRole("grid")).toBeVisible();
  await expect(page.locator('[role="row"][aria-rowindex]').first()).toContainText("pod-000000");
});

const copyCalls = (page: Page) =>
  page.evaluate(() => (window as any).__KV_MOCK__.calls.filter((c: any) => c.cmd === "copy_log_lines").map((c: any) => c.args));

async function dragSelect(page: Page, fromLine: number, toLine: number) {
  const a = await page.locator(`[data-line="${fromLine}"] [data-part=text]`).boundingBox();
  const b = await page.locator(`[data-line="${toLine}"] [data-part=text]`).boundingBox();
  await page.mouse.move(a!.x + 5, a!.y + 9);
  await page.mouse.down();
  await page.mouse.move(b!.x + 60, b!.y + 9, { steps: 6 });
  await page.mouse.up();
}

test("copying a multi-line selection is served by the backend as clean lines", async ({ page }) => {
  await dragSelect(page, 49990, 49996);
  await page.keyboard.press("Meta+c");
  await expect(page.getByText("Copied 7 lines")).toBeVisible();
  expect(await copyCalls(page)).toEqual([expect.objectContaining({ start: 49989, end: 49995, timeMode: "local" })]);
});

test("a selection survives live updates and pauses following", async ({ page }) => {
  await dragSelect(page, 49990, 49996);
  const before = await page.evaluate(() => window.getSelection()!.toString());
  expect(before).toContain("app line 49990");
  await page.evaluate(() => (window as any).__KV_MOCK__.appendLogs(5));
  await expect(page.getByRole("button", { name: "↓ 5 new lines" })).toBeVisible();
  expect(await page.evaluate(() => window.getSelection()!.toString())).toBe(before);
});

test("gutter range selection reaches far beyond the screen; ⌘A selects everything", async ({ page }) => {
  await page.locator('[data-line="50000"]').getByTitle(/Click to select the line/).click();
  await expect(page.getByText("1 line selected")).toBeVisible();
  await page.getByLabel("Log lines").focus();
  await page.keyboard.press("Meta+ArrowUp");
  await page.locator('[data-line="1"]').getByTitle(/Click to select the line/).click({ modifiers: ["Shift"] });
  await expect(page.getByText("50,000 lines selected")).toBeVisible();
  await page.keyboard.press("Meta+c");
  await expect(page.getByText("Copied 50,000 lines")).toBeVisible();
  expect((await copyCalls(page)).at(-1)).toMatchObject({ start: 0, end: 49999 });

  await page.keyboard.press("Escape");
  await expect(page.getByText(/lines? selected/)).toHaveCount(0);
  await page.getByLabel("Log lines").focus();
  await page.keyboard.press("Meta+a");
  await expect(page.getByText("50,000 lines selected")).toBeVisible();
});

test("a selection inside one message uses the native clipboard", async ({ page }) => {
  await page.locator('[data-line="49995"] [data-part=text]').dblclick();
  expect(await page.evaluate(() => window.getSelection()!.toString())).not.toBe("");
  await page.keyboard.press("Meta+c");
  await page.waitForTimeout(200);
  expect(await copyCalls(page)).toEqual([]);
});

test("Escape clears search or line selection without closing the panel", async ({ page }) => {
  const search = page.getByLabel("Search logs");
  await search.fill("error");
  await search.press("Escape");
  await expect(search).toHaveValue("");
  await expect(page.getByRole("tab", { name: "logs" })).toBeVisible();
  // With nothing to clear, Escape still closes the panel.
  await search.press("Escape");
  await expect(page.getByRole("tab", { name: "logs" })).toHaveCount(0);
});
