import { OPEN_API_URL, ASYNC_API_URL } from "./urls";

export const DEFAULT_PREVIEW_NAME = "master";
export const PREVIEW_NAME = "my-preview";
export const SERVICE_NAME = "whoami";
export const mockedApps = {
  [PREVIEW_NAME]: {
    status: "deployed",
    services: [
      {
        name: SERVICE_NAME,
        url: `http://localhost:9001/${PREVIEW_NAME}/${SERVICE_NAME}/`,
        type: "service",
        state: { status: "running" },
        openApiUrl: OPEN_API_URL,
        asyncApiUrl: ASYNC_API_URL,
      },
    ],
  },
};

export function appsAsEventStream(apps) {
  // We need to use this format because the apps are fetched using event streams
  return `
data:${JSON.stringify(apps)}
:


`; // The empty lines at the end are important. Do not delete them!
}

export const mockedAppsAsEventStream = appsAsEventStream(mockedApps);
