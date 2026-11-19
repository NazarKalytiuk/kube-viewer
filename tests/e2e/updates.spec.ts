import { expect, test } from "@playwright/test";
import { installMockBackend, type MockOptions } from "./mockBackend";

const open = async (page: import("@playwright/test").Page, options: Partial<MockOptions>) => {
  await page.addInitScript(installMockBackend, { podCount: 10, ...options });
  await page.goto("/");
  await expect(page.getByText("KubeViewer 0.1.0")).toBeVisible();
};
const calls = (page: import("@playwright/test").Page) =>
  page.evaluate(() => ((window as any).__KV_MOCK__.calls as { cmd: string }[]).map((c) => c.cmd));

test("reports when the app is up to date", async ({ page }) => {
  await open(page, {});
  await page.getByRole("button", { name: "Check for updates" }).click();
  await expect(page.getByText("You have the latest version.")).toBeVisible();
});

test("downloads, installs and restarts only on request", async ({ page }) => {
  await open(page, { update: { version: "0.2.0", notes: "Faster logs\nUniversal build" } });
  await page.getByRole("button", { name: "Check for updates" }).click();
  await expect(page.getByText("Version 0.2.0 is available")).toBeVisible();
  await page.getByText("What’s new").click();
  await expect(page.getByText("Universal build")).toBeVisible();

  await page.getByRole("button", { name: "Update", exact: true }).click();
  // Late progress events must not hide the finished state.
  await expect(page.getByText("Version 0.2.0 is installed. Restart to use it.")).toBeVisible();
  await page.waitForTimeout(50);
  await expect(page.getByText("Version 0.2.0 is installed. Restart to use it.")).toBeVisible();
  expect(await calls(page)).not.toContain("plugin:process|restart");

  await page.getByRole("button", { name: "Restart" }).click();
  await expect.poll(() => calls(page)).toContain("plugin:process|restart");
});

test("shows install failures and allows checking again", async ({ page }) => {
  await open(page, { update: { version: "0.2.0", fail: "signature verification failed" } });
  await page.getByRole("button", { name: "Check for updates" }).click();
  await page.getByRole("button", { name: "Update", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("Update failed: signature verification failed");
  await page.getByRole("button", { name: "Check for updates" }).click();
  await expect(page.getByText("Version 0.2.0 is available")).toBeVisible();
});
