import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? "github" : "list",
  use: {
    baseURL: "http://localhost:1430",
    trace: "retain-on-failure",
    viewport: { width: 1440, height: 900 },
  },
  projects: [{ name: "webkit", use: { ...devices["Desktop Safari"], viewport: { width: 1440, height: 900 } } }],
  webServer: {
    command: "pnpm dev",
    url: "http://localhost:1430",
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
  },
});
