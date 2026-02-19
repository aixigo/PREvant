import { test } from "@playwright/test";
import {
  DEFAULT_PREVIEW_NAME,
  PREVIEW_NAME,
  appsAsEventStream,
  mockedApps,
} from "./fixtures/apps";
import { injectGlobalOverride } from "./util/injectGlobalOverrides";
import {
  expectAppActionHidden,
  expectAppActionVisible,
} from "./util/appActions";

function appsWithDefaultAndPreview() {
  return {
    ...mockedApps,
    [DEFAULT_PREVIEW_NAME]: {
      ...mockedApps[PREVIEW_NAME],
    },
  };
}

test.describe("default app config", () => {
  test.beforeEach(async ({ page }) => {
    await page.route("**/api/apps", (route) => {
      route.fulfill({
        status: 200,
        contentType: "text/event-stream;charset=UTF-8",
        body: appsAsEventStream(appsWithDefaultAndPreview()),
      });
    });
  });

  test("should hide shutdown action for configured default app", async ({
    page,
  }) => {
    await injectGlobalOverride(page, "config", {
      defaultAppName: DEFAULT_PREVIEW_NAME,
    });
    await page.goto("/");

    await expectAppActionHidden({
      page,
      appName: DEFAULT_PREVIEW_NAME,
      action: "Shutdown",
    });
    await expectAppActionVisible({
      page,
      appName: PREVIEW_NAME,
      action: "Shutdown",
    });
  });

  test("should hide shutdown action only for the app matching configured default name", async ({
    page,
  }) => {
    await injectGlobalOverride(page, "config", {
      defaultAppName: PREVIEW_NAME,
    });
    await page.goto("/");

    await expectAppActionHidden({
      page,
      appName: PREVIEW_NAME,
      action: "Shutdown",
    });
    await expectAppActionVisible({
      page,
      appName: DEFAULT_PREVIEW_NAME,
      action: "Shutdown",
    });
  });
});
