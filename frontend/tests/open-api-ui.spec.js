import { test, expect } from "@playwright/test";
import { OPEN_API_URL } from "./fixtures/urls";

// Smoke test: ensures the OpenApiUI loads correctly and its overall
// visual appearance matches the expected baseline. This helps detect
// unexpected UI regressions caused by code, dependency or style changes.
test("smoke test - open api ui looks the same", async ({ page }) => {
  await page.goto(`/#/open-api-ui/${encodeURIComponent(OPEN_API_URL)}`);

  await expect(
    page.getByRole("heading", {
      name: `API Documentation`,
    }),
    "OpenApi dialog should be visible"
  ).toBeVisible();

  await expect(page).toHaveScreenshot("open-api-ui.png", {
    fullPage: true, 
  });
});
