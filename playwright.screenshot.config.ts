import { defineConfig, devices } from '@playwright/test';

const cargo = process.env.CARGO ?? `${process.env.HOME}/.cargo/bin/cargo`;
const pathWithCargo = [
  `${process.env.HOME}/.cargo/bin`,
  process.env.PATH ?? '',
].join(':');
const port = process.env.EASYTEX_SCREENSHOT_PORT ?? '8082';

export default defineConfig({
  testDir: './screenshot',
  timeout: 120000,
  fullyParallel: false,
  workers: 1,
  reporter: 'list',
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    ...devices['Desktop Chrome'],
    viewport: { width: 1440, height: 960 },
    trace: 'off',
    screenshot: 'off',
  },
  webServer: {
    command: `${cargo} run -- serve tests --config tests/easytex.yaml`,
    url: `http://127.0.0.1:${port}/health`,
    reuseExistingServer: false,
    env: {
      PORT: port,
      PATH: pathWithCargo,
      EASYTEX_REQUIRE_AUTH: 'false',
    },
    timeout: 120000,
    stdout: 'pipe',
    stderr: 'pipe',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
});
