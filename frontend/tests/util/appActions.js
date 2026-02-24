import { expect } from "@playwright/test";

export async function openAppMenu({ page, appName }) {
  await page.click(
    `div.card:has(.card-header:has-text("${appName}")) button[data-toggle="dropdown"]`
  );
}

export function appActionButton({ page, appName, action }) {
  return page.locator(
    `div.card:has(.card-header:has-text("${appName}")) button:text("${action}")`
  );
}

export async function expectAppActionVisible({ page, appName, action }) {
  await openAppMenu({ page, appName });
  await expect(
    appActionButton({ page, appName, action }),
    `${action} action should be visible`
  ).toBeVisible();
}

export async function expectAppActionHidden({ page, appName, action }) {
  await openAppMenu({ page, appName });
  await expect(
    appActionButton({ page, appName, action }),
    `${action} action should not be visible`
  ).not.toBeVisible();
}
