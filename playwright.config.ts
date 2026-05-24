import { defineConfig, devices } from '@playwright/test';

const cargo = process.env.CARGO ?? `${process.env.HOME}/.cargo/bin/cargo`;
const pathWithCargo = [
  `${process.env.HOME}/.cargo/bin`,
  process.env.PATH ?? '',
].join(':');
const e2eAdminToken = process.env.EASYTEX_E2E_ADMIN_TOKEN ?? process.env.EASYTEX_ADMIN_TOKEN ?? '';

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
    command: `${cargo} run -- serve tests --config tests/easytex.yaml`,
    url: 'http://127.0.0.1:8081/health',
    reuseExistingServer: !process.env.CI,
    env: {
      PORT: '8081',
      PATH: pathWithCargo,
      EASYTEX_ADMIN_TOKEN: e2eAdminToken,
      EASYTEX_REQUIRE_AUTH: e2eAdminToken ? 'true' : 'false',
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
