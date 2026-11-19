import { test } from "@playwright/test";
import { installMockBackend } from "./mockBackend";

// Not part of the suite: run with SCREENS=1 to capture review screenshots.
test.skip(!process.env.SCREENS, "screenshots only on demand");

for (const scheme of ["light", "dark"] as const) {
  test(`screens ${scheme}`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: scheme });
    await page.addInitScript(installMockBackend, { podCount: 100000 });
    await page.goto("/");
    await page.locator('[role="row"][aria-rowindex]').first().waitFor();
    await page.screenshot({ path: `${process.env.SCREENS}/table-${scheme}.png` });
    await page.locator('[role="row"][aria-rowindex]').nth(0).click();
    await page.getByText("Conditions").waitFor();
    await page.screenshot({ path: `${process.env.SCREENS}/detail-${scheme}.png` });
    await page.getByRole("tab", { name: "YAML" }).click();
    await page.locator(".cm-content").waitFor();
    await page.screenshot({ path: `${process.env.SCREENS}/yaml-${scheme}.png` });
    await page.getByRole("tab", { name: "logs" }).click();
    await page.getByText("5,000 lines").waitFor();
    await page.screenshot({ path: `${process.env.SCREENS}/logs-${scheme}.png` });
  });
}
