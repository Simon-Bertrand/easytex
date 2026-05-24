import { expect, test } from '@playwright/test';
import { mkdirSync } from 'node:fs';

test('capture README screenshot', async ({ page }) => {
  await page.goto('/demo');
  await page.addStyleTag({
    content: `
      *, *::before, *::after {
        animation-duration: 0s !important;
        animation-delay: 0s !important;
        transition-duration: 0s !important;
        caret-color: transparent !important;
      }
    `,
  });

  await expect(page.locator('header')).toContainText('demo', { timeout: 15000 });
  await expect(page.locator('#l')).toContainText(/Connected to server/, {
    timeout: 30000,
  });
  await expect(page.locator('canvas').first()).toBeVisible({ timeout: 60000 });
  await page.waitForTimeout(500);

  mkdirSync('docs', { recursive: true });
  await page.screenshot({
    path: 'docs/readme-screenshot.png',
    fullPage: false,
  });
});
