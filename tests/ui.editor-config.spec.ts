import { expect, test } from '@playwright/test';
import { installUiToken, openProject, saveProjectFile, withProject } from './e2e-helpers';

test.describe('UI editor and config', () => {
  test.beforeEach(async ({ page }) => {
    await installUiToken(page);
  });

  test('shows project files and opens the settings modal errors', async ({ page, request }) => {
    await withProject(request, async (project) => {
      await saveProjectFile(request, project, 'chapters/intro.tex', 'Intro from API');
      await openProject(page, project);

      await expect(page.locator('.file-item', { hasText: 'main.tex' })).toBeVisible({ timeout: 15000 });
      await expect(page.locator('.file-item', { hasText: 'chapters' })).toBeVisible({ timeout: 15000 });

      await page.click('button:has-text("Settings")');
      await expect(page.locator('#modal')).toBeVisible();
      await page.locator('#cfg-txt').fill('entrypoint = "../main.tex"');
      await page.click('button:has-text("Save Changes")');
      await expect(page.locator('#cfg-err')).toContainText(/entrypoint|Invalid/i);
    }, 'ui-config');
  });

  test('creates a project from the dashboard and renders its project shell', async ({ page, request }) => {
    await installUiToken(page);
    const name = `ui-new-${Date.now()}`;
    page.on('dialog', async dialog => dialog.accept(name));

    await page.goto('/');
    await page.click('button:has-text("New Project")');
    await expect(page).toHaveURL(new RegExp(`/${name}`));
    await expect(page.locator('header')).toContainText(name);

    await request.post(`/api/delete/${encodeURIComponent(name)}`, {
      headers: { 'X-EasyTex-Request': 'true', ...(process.env.EASYTEX_E2E_ADMIN_TOKEN ? { Authorization: `Bearer ${process.env.EASYTEX_E2E_ADMIN_TOKEN}` } : {}) },
    });
  });
});
