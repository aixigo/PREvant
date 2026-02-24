import { test, expect } from "@playwright/test";
import { PREVIEW_NAME, appsAsEventStream, mockedApps } from "./fixtures/apps";
import { injectGlobalOverride } from "./util/injectGlobalOverrides";
import { interceptAppsApiCall } from "./util/interceptApiCalls";
import { expectAppActionHidden, expectAppActionVisible } from "./util/appActions";

test.beforeEach(interceptAppsApiCall);

test.describe("when backups are disabled", () => {
  test.beforeEach(async ({ page }) => {
    await injectGlobalOverride(page, "config", {
      isAuthRequired: false,
      isBackupsEnabled: false,
    });
    await page.goto("/");
  });

  test("should not render backup action", async ({ page }) => {
    await expectAppActionHidden({ page, appName: PREVIEW_NAME, action: "Back up" });
  });
});

test.describe("when backups are enabled", () => {
  test.beforeEach(async ({ page }) => {
    await injectGlobalOverride(page, "config", {
      isAuthRequired: false,
      isBackupsEnabled: true,
    });
    await page.goto("/");
  });

  test("should render backup action", async ({ page }) => {
    await expectAppActionVisible({ page, appName: PREVIEW_NAME, action: "Back up" });
  });
});

test.describe("when backups are enabled and app is backed up", () => {
  test.beforeEach(async ({ page }) => {
    await injectGlobalOverride(page, "config", {
      isAuthRequired: false,
      isBackupsEnabled: true,
    });
    await page.route("**/api/apps", (route) => {
      route.fulfill({
        status: 200,
        contentType: "text/event-stream;charset=UTF-8",
        body: appsAsEventStream({
          ...mockedApps,
          [PREVIEW_NAME]: {
            ...mockedApps[PREVIEW_NAME],
            status: "backed-up",
          },
        }),
      });
    });
    await page.goto("/");
  });

  test("should render redeploy action", async ({ page }) => {
    await expectAppActionVisible({
      page,
      appName: PREVIEW_NAME,
      action: "Redeploy",
    });
  });
});
