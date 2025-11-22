import { test, expect } from "@playwright/test";
import { PREVIEW_NAME } from "./fixtures/apps";
import { issuers, me } from "./fixtures/auth";
import { injectGlobalOverride } from "./util/injectGlobalOverrides";
import { interceptAppsApiCall } from "./util/interceptApiCalls";

test.beforeEach(interceptAppsApiCall);

test.describe("when no issuers are configured", () => {
  test.beforeEach(async ({ page }) => {
    await injectGlobalOverride(page, "issuers", null);
    await page.goto("/");
  });

  test("should not render a login button", expectNoLoginButton);
});

test.describe("when at least one issuer is configured", () => {
  test.beforeEach(async ({ page }) => {
    await injectGlobalOverride(page, "issuers", issuers);
    await page.goto("/");
  });

  test("should render a login button", expectLoginButton);
});

test.describe("when the user is logged in", () => {
  test.beforeEach(async ({ page }) => {
    await injectGlobalOverride(page, "me", me);
    await injectGlobalOverride(page, "issuers", issuers);
    await page.goto("/");
  });

  test("should not render a login button", expectNoLoginButton);

  test("should display the name of the user", async ({ page }) => {
    await expect(
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

  test("should allow shutting down apps", shouldAllowShuttingDownApp);

  test("should allow duplicating apps", shouldAllowDuplicatingApp);
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

    test("should not allow shutting down apps", shouldNotAllowShuttingDownApp);

    test("should not allow duplicating apps", shouldNotAllowDuplicatingApp);
  });

  test.describe("and the user is logged in", () => {
    test.beforeEach(async ({ page }) => {
      await injectGlobalOverride(page, "me", me);

      await page.goto("/");
    });

    test("should allow shutting down apps", shouldAllowShuttingDownApp);

    test("should allow duplicating apps", shouldAllowDuplicatingApp);
  });
});

async function expectLoginButton({ page }) {
  await expect(
    page.locator('a:has-text("Login with")'),
    "button with 'Login with' text exists"
  ).toBeVisible();
}

async function expectNoLoginButton({ page }) {
  await expect(
    page.locator('a:has-text("Login with")'),
    "no button with 'Login with' text exists"
  ).not.toBeVisible();
}

async function openDialogViaMenu({ page, action }) {
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

async function shouldAllowDuplicatingApp({ page }) {
  await shouldAllowActionOnApp({ page, action: "Duplicate" });
}

async function shouldAllowShuttingDownApp({ page }) {
  await shouldAllowActionOnApp({ page, action: "Shutdown" });
}

async function shouldAllowActionOnApp({ page, action }) {
  const dialog = await openDialogViaMenu({ page, action });
  const confirmButtonText = getConfirmButtonText({ action });

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

async function shouldNotAllowDuplicatingApp({ page }) {
  await shouldNotAllowActionOnApp({ page, action: "Duplicate" });
}

async function shouldNotAllowShuttingDownApp({ page }) {
  await shouldNotAllowActionOnApp({ page, action: "Shutdown" });
}

async function shouldNotAllowActionOnApp({ page, action }) {
  const dialog = await openDialogViaMenu({ page, action });
  const confirmButtonText = getConfirmButtonText({ action });

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

function getConfirmButtonText({ action }) {
  return action === "Duplicate" ? "Duplicate" : "Confirm";
}
