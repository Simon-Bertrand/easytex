import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  timeout: 90000,
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: 1,
  reporter: 'html',
  use: {
    baseURL: 'http://127.0.0.1:8081',
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
  },
  webServer: {
    command: 'cargo run -- serve tests',
    url: 'http://127.0.0.1:8081/health',
    reuseExistingServer: !process.env.CI,
    env: {
      PORT: '8081',
      PATH: process.env.PATH ?? '',
    },
    timeout: 120000,
    stdout: 'inherit',
    stderr: 'inherit',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
});
