import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  webServer: {
    command: 'npm run serve', 
    port: 9001,
    reuseExistingServer: !process.env.CI,
  },
  use: {
    baseURL: 'http://localhost:9001',
    headless: true,
  },
});
