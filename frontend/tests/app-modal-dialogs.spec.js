import { test, expect } from "@playwright/test";

const PREVIEW_NAME = "master";
const SERVICE_NAME = "whoami";
const FILTER_STRING = PREVIEW_NAME.substring(0, 4);
const mockedApps = {
  [PREVIEW_NAME]: [
    {
      name: SERVICE_NAME,
      url: `http://localhost:9001/${PREVIEW_NAME}/${SERVICE_NAME}/`,
      type: "service",
      state: { status: "running" },
      openApiUrl: `http://localhost:9001/${PREVIEW_NAME}/${SERVICE_NAME}/swagger.json`,
      asyncApiUrl: `http://localhost:9001/${PREVIEW_NAME}/${SERVICE_NAME}/asyncApi.json`,
    },
  ],
};

// We need to use this format because the apps are fetched using event streams
const mockedAppsAsEventStream = `
data:${JSON.stringify(mockedApps)}
:


`; // The empty lines at the end are important. Do not delete them!

test.describe("app modal dialogs", () => {
  test.beforeEach(async ({ page }) => {
    await page.route("**/api/apps", (route) => {
      route.fulfill({
        status: 200,
        contentType: "text/event-stream;charset=UTF-8",
        body: mockedAppsAsEventStream,
      });
    });
  });

  test.describe("when entering through the home page and filtering the apps", () => {
    test.beforeEach(async ({ page }) => {
      await page.goto("/");

      // Wait for preview and service
      await expect(
        page.getByRole("heading", { name: PREVIEW_NAME })
      ).toBeVisible();
      await expect(page.getByText(SERVICE_NAME)).toBeVisible();

      // Apply filter
      const filterInput = page.getByPlaceholder("Search Apps");
      await filterInput.fill(FILTER_STRING);

      // Ensure preview and service are still visible after filter
      await expect(
        page.getByRole("heading", { name: PREVIEW_NAME })
      ).toBeVisible();
      await expect(page.getByText(SERVICE_NAME)).toBeVisible();
    });

    test.afterEach(async ({ page }) => {
      // Close modal
      await page.getByLabel("Close").click();

      // Ensure query param is preserved
      await expect(page).toHaveURL(
        new RegExp(`.*\\?appNameFilter=${FILTER_STRING}.*`)
      );
    });

    test("should retain query param after closing Logs dialog", async ({
      page,
    }) => {
      // open Logs
      await page.click(
        `div.card:has(.card-header:has-text("${PREVIEW_NAME}")):has(.card-body a:text("${SERVICE_NAME}")) a:text("Logs")`
      );

      // make sure dialog is visible
      await expect(
        page.getByRole("heading", {
          name: `Logs of ${SERVICE_NAME} in ${PREVIEW_NAME}`,
        })
      ).toBeVisible();
    });

    test("should retain query param after closing Open API Documentation dialog", async ({
      page,
    }) => {
      // open Open API Documentation
      await page.click(
        `div.card:has(.card-header:has-text("${PREVIEW_NAME}")):has(.card-body a:text("${SERVICE_NAME}")) a:text("Open API Documentation")`
      );

      // make sure dialog is visible
      await expect(
        page.getByRole("heading", {
          name: `API Documentation`,
        })
      ).toBeVisible();
    });

    test("should retain query param after closing Async API Documentation dialog", async ({
      page,
    }) => {
      // open Async API Documentation
      await page.click(
        `div.card:has(.card-header:has-text("${PREVIEW_NAME}")):has(.card-body a:text("${SERVICE_NAME}")) a:text("Async API Documentation")`
      );

      // make sure dialog is visible
      await expect(
        page.getByRole("heading", {
          name: "AsyncAPI Documentation",
        })
      ).toBeVisible();
    });
  });

  test.describe("when opening the Logs dialog via direct URL", () => {
    test("should navigate to home when closing the dialog and not close the tab or navigate to a 3rd party site", async ({
      page,
    }) => {
      // Go directly to logs dialog URL
      await page.goto(`/#/logs/${PREVIEW_NAME}/${SERVICE_NAME}`);

      // Make sure logs dialog is visible
      await expect(
        page.getByRole("heading", {
          name: `Logs of ${SERVICE_NAME} in ${PREVIEW_NAME}`,
        })
      ).toBeVisible();

      // Close the dialog
      await page.getByLabel("Close").click();

      // Ensure we're on the home page (no previous page fallback, no tab close)
      await expect(page).toHaveURL(/\/#\/$/);
    });
  });
});
