import { mockedAppsAsEventStream } from "../fixtures/apps";

export async function interceptAppsApiCall({ page }) {
  await page.route("**/api/apps", (route) => {
    route.fulfill({
      status: 200,
      contentType: "text/event-stream;charset=UTF-8",
      body: mockedAppsAsEventStream,
    });
  });
}
