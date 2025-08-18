import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  snapshotDir: "./tests/snapshots",
  reporter: [
    ["list"],
    ["html", { open: "never" }],
    ...(process.env.CI ? [["github"]] : []),
  ],
  webServer: {
    command: "npm run serve",
    port: 9001,
    reuseExistingServer: !process.env.CI,
  },
  use: {
    baseURL: "http://localhost:9001",
    headless: true,
    viewport: { width: 1280, height: 720 }, // define a fixed viewports to ensure screenshots always match
  },
});
