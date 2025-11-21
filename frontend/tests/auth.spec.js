import { test, expect } from "@playwright/test";
import { PREVIEW_NAME, mockedAppsAsEventStream } from "./fixtures/apps";
import { issuers, me } from "./fixtures/auth";
import { injectGlobalOverride } from "./util/injectGlobalOverrides";

test.beforeEach(async ({ page }) => {
  await page.route("**/api/apps", (route) => {
    route.fulfill({
      status: 200,
      contentType: "text/event-stream;charset=UTF-8",
      body: mockedAppsAsEventStream,
    });
  });
});

test.describe("when no issuers are configured", () => {
  test.beforeEach(async ({ page }) => {
    await injectGlobalOverride(page, "issuers", null);
    await page.goto("/");
  });

  test("should not render a login button", async ({ page }) => {
    await expectNoLoginButton(page);
  });
});

test.describe("when at least one issuer is configured", () => {
  test.beforeEach(async ({ page }) => {
    await injectGlobalOverride(page, "issuers", issuers);
    await page.goto("/");
  });

  test("should render a login button", async ({ page }) => {
    await expectLoginButton(page);
  });
});

test.describe("when the user is logged in", () => {
  test.beforeEach(async ({ page }) => {
    await injectGlobalOverride(page, "me", me);
    await injectGlobalOverride(page, "issuers", issuers);
    await page.goto("/");
  });

  test("should not render a login button", async ({ page }) => {
    await expectNoLoginButton(page);
  });

  test("should display the name of the user", async ({ page }) => {
    expect(
      page.locator(`a:has-text("${me.name}")`),
      "name of the logged in user is displayed"
    ).toBeVisible();
  });
});

test.describe("when auth is not required", () => {
  test.beforeEach(async ({ page }) => {
    await injectGlobalOverride(page, "config", { isAuthRequired: false });
    await page.goto("/");
  });

  test("should allow shutting down apps", async ({ page }) => {
    await shouldAllowShuttingDownApp(page);
  });

  test("should allow duplicating apps", async ({ page }) => {
    await shouldAllowDuplicatingApp(page);
  });
});

test.describe("when auth is required", () => {
  test.beforeEach(async ({ page }) => {
    await injectGlobalOverride(page, "config", { isAuthRequired: true });
  });

  test.describe("and the user is not logged in", () => {
    test.beforeEach(async ({ page }) => {
      await injectGlobalOverride(page, "me", null);

      await page.goto("/");
    });

    test("should not allow shutting down apps", async ({ page }) => {
      await shouldNotAllowShuttingDownApp(page);
    });

    test("should not allow duplicating apps", async ({ page }) => {
      await shouldNotAllowDuplicatingApp(page);
    });
  });

  test.describe("and the user is logged in", () => {
    test.beforeEach(async ({ page }) => {
      await injectGlobalOverride(page, "me", me);

      await page.goto("/");
    });

    test("should allow shutting down apps", async ({ page }) => {
      await shouldAllowShuttingDownApp(page);
    });

    test("should allow duplicating apps", async ({ page }) => {
      await shouldAllowDuplicatingApp(page);
    });
  });
});

async function expectLoginButton(page) {
  await expect(
    page.locator('a:has-text("Login with")'),
    "button with 'Login with' text exists"
  ).toBeVisible();
}

async function expectNoLoginButton(page) {
  await expect(
    page.locator('a:has-text("Login with")'),
    "no button with 'Login with' text exists"
  ).not.toBeVisible();
}

async function openDialogViaMenu(page, action) {
  await page.click(
    `div.card:has(.card-header:has-text("${PREVIEW_NAME}")) button[data-toggle="dropdown"]`
  );
  await page.click(
    `div.card:has(.card-header:has-text("${PREVIEW_NAME}")) button:text("${action}")`
  );

  const dialog = page
    .getByRole("dialog")
    .filter({ hasText: `${action} ${PREVIEW_NAME}` });
  await expect(dialog, `${action} dialog should be visible`).toBeVisible();
  return dialog;
}

async function shouldAllowDuplicatingApp(page) {
  await shouldAllowActionOnApp(page, "Duplicate");
}

async function shouldAllowShuttingDownApp(page) {
  await shouldAllowActionOnApp(page, "Shutdown");
}

async function shouldAllowActionOnApp(page, action) {
  const dialog = await openDialogViaMenu(page, action);
  const confirmButtonText = getConfirmButtonText(action);

  await expect(
    page.getByText(`To ${action} an app you need to be logged in.`),
    "login required message is not shown"
  ).not.toBeVisible();

  await expect(
    page.getByRole("button", { name: confirmButtonText }),
    "confirm button should initially be disabled"
  ).toBeDisabled();

  const input = dialog.getByPlaceholder("Enter app name");
  await expect(input, "input should not be disabled").not.toBeDisabled();
  await input.fill(PREVIEW_NAME);

  await expect(
    page.getByRole("button", { name: confirmButtonText }),
    "confirm button should be enabled"
  ).not.toBeDisabled();
}

async function shouldNotAllowDuplicatingApp(page) {
  await shouldNotAllowActionOnApp(page, "Duplicate");
}

async function shouldNotAllowShuttingDownApp(page) {
  await shouldNotAllowActionOnApp(page, "Shutdown");
}

async function shouldNotAllowActionOnApp(page, action) {
  const dialog = await openDialogViaMenu(page, action);
  const confirmButtonText = getConfirmButtonText(action);

  await expect(
    page.getByText(`To ${action} an app you need to be logged in.`),
    "login required message is shown"
  ).toBeVisible();

  await expect(
    page.getByRole("button", { name: confirmButtonText }),
    "confirm button should be disabled"
  ).toBeDisabled();

  const input = dialog.getByPlaceholder("Enter app name");
  await expect(input, "input should be disabled").toBeDisabled();
}

function getConfirmButtonText(action) {
  return action === "Duplicate" ? "Duplicate" : "Confirm";
}
