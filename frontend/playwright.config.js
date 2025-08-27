import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
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
  },
});
