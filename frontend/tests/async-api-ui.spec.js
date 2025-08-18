import { test, expect } from "@playwright/test";
import { ASYNC_API_URL } from "./fixtures/urls";

// Smoke test: ensures the AsyncApiUi loads correctly and its overall
// visual appearance matches the expected baseline. This helps detect
// unexpected UI regressions caused by code, dependency or style changes.
test("smoke test - async api ui looks the same", async ({ page }) => {
  await page.goto(`/#/async-api-ui/${encodeURIComponent(ASYNC_API_URL)}`);

  await expect(
    page.getByRole("heading", {
      name: "AsyncAPI Documentation",
    }),
    "AsyncAPI dialog should be visible"
  ).toBeVisible();

  await expect(page).toHaveScreenshot("async-api-ui.png", {
    fullPage: true,
  });
});
