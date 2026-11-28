import { expect, test } from "@playwright/test";
import { installMockBackend } from "./mockBackend";

const POD_COUNT = 100_000;

test.beforeEach(async ({ page }) => {
  await page.addInitScript(installMockBackend, { podCount: POD_COUNT });
  await page.goto("/");
});

const rows = (page: import("@playwright/test").Page) => page.locator('[role="row"][aria-rowindex]');

test("renders 100k pods with a bounded DOM", async ({ page }) => {
  await expect(page.getByRole("heading", { name: "Pods" })).toBeVisible();
  await expect(page.getByText("100,000", { exact: true })).toBeVisible();
  await expect(rows(page).first()).toContainText("pod-000000");
  const domRows = await rows(page).count();
  expect(domRows).toBeGreaterThan(10);
  expect(domRows).toBeLessThan(120);
});

test("scrolls to the end without growing the DOM", async ({ page }) => {
  const grid = page.getByRole("grid");
  await grid.focus();
  await page.keyboard.press("End");
  await expect(page.locator('[role="row"][aria-selected="true"]')).toContainText("pod-099999");
  expect(await rows(page).count()).toBeLessThan(120);
});

test("filters and sorts on the backend", async ({ page }) => {
  await page.getByLabel("Filter").first().fill("pod-00001");
  await expect(page.getByText("10 of 100,000")).toBeVisible();
  await expect(rows(page)).toHaveCount(10);

  const nameHeader = page.getByRole("columnheader", { name: "Name", exact: true });
  await nameHeader.click();
  await expect(nameHeader).toHaveAttribute("aria-sort", "descending");
  await expect(rows(page).first()).toContainText("pod-000019");
});

test("filtering never shows the previous rows under the new count", async ({ page }) => {
  await expect(rows(page).first()).toContainText("pod-000000");
  await page.evaluate(() => {
    const w = window as any;
    w.__KV_MOCK__.setLatency(30);
    // Count painted frames with the filtered row count but rows that don't match.
    w.__staleFrames = 0;
    const sample = () => {
      const shown = [...document.querySelectorAll('[role="row"][aria-rowindex]')];
      if (shown.length > 0 && shown.length <= 10 && shown.some((r) => !r.textContent?.includes("pod-00001"))) w.__staleFrames++;
      requestAnimationFrame(sample);
    };
    requestAnimationFrame(sample);
  });

  await page.getByLabel("Filter").first().fill("pod-00001");
  await expect(page.getByText("10 of 100,000")).toBeVisible();
  await expect(rows(page)).toHaveCount(10);
  await expect(rows(page).first()).toContainText("pod-000010");
  expect(await page.evaluate(() => (window as any).__staleFrames as number)).toBe(0);
});

test("opens details, YAML and logs for a pod", async ({ page }) => {
  await rows(page).nth(2).click();
  await expect(page.getByRole("button", { name: "Close details" })).toBeVisible();
  await expect(page.getByRole("tab", { name: "status" })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByText("Conditions")).toBeVisible();

  await page.getByRole("tab", { name: "YAML" }).click();
  await expect(page.locator(".cm-content")).toContainText("name: pod-000002");

  await page.getByRole("tab", { name: "logs" }).click();
  // Default history: the last 1,000 lines.
  await expect(page.getByText(/^1,000 lines/)).toBeVisible();

  await page.getByRole("button", { name: "Close details" }).click();
  await expect(page.getByRole("tab", { name: "status" })).toHaveCount(0);
});

test("a failing context shows an actionable error and keeps the sidebar usable", async ({ page }) => {
  await expect(rows(page).first()).toBeVisible();
  await page.getByLabel("Context").selectOption("prod-eks");
  await expect(page.getByText("Cannot connect to prod-eks")).toBeVisible();
  await expect(page.getByText(/log in again in a terminal/)).toBeVisible();
  await expect(page.getByText("exec: aws")).toBeVisible();

  await page.getByLabel("Context").selectOption("kind-dev");
  await expect(rows(page).first()).toContainText("pod-000000");
});
