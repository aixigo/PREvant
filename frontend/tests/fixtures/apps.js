import { OPEN_API_URL, ASYNC_API_URL } from './urls'

export const DEFAULT_PREVIEW_NAME = "master";
export const PREVIEW_NAME = "my-preview";
export const SERVICE_NAME = "whoami";
export const mockedApps = {
  [PREVIEW_NAME]: {
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

// We need to use this format because the apps are fetched using event streams
export const mockedAppsAsEventStream = `
data:${JSON.stringify(mockedApps)}
:


`; // The empty lines at the end are important. Do not delete them!