import { test, expect } from "@playwright/test";
import { PREVIEW_NAME, SERVICE_NAME } from "./fixtures/apps";
import { interceptAppsApiCall } from "./util/interceptApiCalls";

const FILTER_STRING = PREVIEW_NAME.substring(0, 4);

test.describe("app modal dialogs", () => {
  test.beforeEach(interceptAppsApiCall);

  test.describe("when entering through the home page and filtering the apps", () => {
    test.beforeEach(async ({ page }) => {
      await page.goto("/");

      await expect(
        page.getByRole("heading", { name: PREVIEW_NAME }),
        `preview "${PREVIEW_NAME}" should be visible after initial loading`
      ).toBeVisible();
      await expect(
        page.getByText(SERVICE_NAME),
        `service "${SERVICE_NAME}" should be visible after initial loading`
      ).toBeVisible();

      // Apply filter
      const filterInput = page.getByPlaceholder("Search Apps");
      await filterInput.fill(FILTER_STRING);

      await expect(
        page.getByRole("heading", { name: PREVIEW_NAME }),
        `preview "${PREVIEW_NAME}" should remain visible after filtering`
      ).toBeVisible();

      await expect(
        page.getByText(SERVICE_NAME),
        `service "${SERVICE_NAME}" should remain visible after filtering`
      ).toBeVisible();
    });

    test.afterEach(async ({ page }) => {
      // Close modal
      await page.getByLabel("Close").click();

      await expect(page, "query parameter should be preserved").toHaveURL(
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

      await expect(
        page.getByRole("heading", {
          name: `Logs of ${SERVICE_NAME} in ${PREVIEW_NAME}`,
        }),
        "logs dialog should be visible"
      ).toBeVisible();
    });

    test("should retain query param after closing Open API Documentation dialog", async ({
      page,
    }) => {
      // open Open API Documentation
      await page.click(
        `div.card:has(.card-header:has-text("${PREVIEW_NAME}")):has(.card-body a:text("${SERVICE_NAME}")) a:text("Open API Documentation")`
      );

      await expect(
        page.getByRole("heading", {
          name: `API Documentation`,
        }),
        "OpenApi dialog should be visible"
      ).toBeVisible();
    });

    test("should retain query param after closing Async API Documentation dialog", async ({
      page,
    }) => {
      // open Async API Documentation
      await page.click(
        `div.card:has(.card-header:has-text("${PREVIEW_NAME}")):has(.card-body a:text("${SERVICE_NAME}")) a:text("Async API Documentation")`
      );

      await expect(
        page.getByRole("heading", {
          name: "AsyncAPI Documentation",
        }),
        "AsyncAPI dialog should be visible"
      ).toBeVisible();
    });
  });

  test.describe("when opening the Logs dialog via direct URL", () => {
    test("should navigate to home when closing the dialog and not close the tab or navigate to a 3rd party site", async ({
      page,
    }) => {
      // Go directly to logs dialog URL
      await page.goto(`/#/logs/${PREVIEW_NAME}/${SERVICE_NAME}`);

      await expect(
        page.getByRole("heading", {
          name: `Logs of ${SERVICE_NAME} in ${PREVIEW_NAME}`,
        }),
        "logs dialog should be visible"
      ).toBeVisible();

      // Close the dialog
      await page.getByLabel("Close").click();

      await expect(
        page,
        "should naviate to home page and not any other previous page or close the tab"
      ).toHaveURL(/\/#\/$/);
    });
  });
});
