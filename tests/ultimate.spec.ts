import { test, expect } from '@playwright/test';

test.describe('EasyTex Ultimate E2E Suite', () => {
  const connected = /Connecté au serveur|Connected to server/;
  
  test('Full Life-Cycle: Dashboard -> Project -> Build -> Stats', async ({ page }) => {
    await page.goto('/');
    await expect(page).toHaveTitle(/EasyTex/);
    
    const projectCard = page.locator('.project-card', { hasText: 'demo' });
    await expect(projectCard).toBeVisible();
    
    await projectCard.click();
    await expect(page).toHaveURL(/\/demo/);
    
    const logContainer = page.locator('#l');
    await expect(logContainer).toContainText(connected, { timeout: 30000 });
    
    const canvas = page.locator('canvas').first();
    await expect(canvas).toBeVisible({ timeout: 45000 });
    
    const runBtn = page.locator('#btn-run');
    await runBtn.click();
    await expect(runBtn).toContainText('Stop');
    await expect(runBtn).toContainText('Run', { timeout: 30000 });
    
    const stats = page.locator('#stats');
    await expect(stats).toContainText('KB');
    await expect(stats).toContainText('s');
    await expect(stats).toContainText('words');
    
    await page.click('text=Settings');
    const modal = page.locator('#modal');
    await expect(modal).toBeVisible();
    
    const textarea = page.locator('#cfg-txt');
    await expect(textarea).toHaveValue(/entrypoint = "main.tex"/);
    
    // Try to save invalid TOML
    await textarea.fill('invalid = "');
    await page.click('text=Save Changes');
    await expect(page.locator('#cfg-err')).toBeVisible();
    await expect(page.locator('#cfg-err')).toContainText('TOML Error');
    
    await page.click('text=Cancel');
    await expect(modal).not.toBeVisible();
  });

  test('Project Creation Workflow', async ({ page, request }) => {
    await page.goto('/');
    
    const newProjectName = `test-project-${Date.now()}`;
    
    page.on('dialog', async dialog => {
      await dialog.accept(newProjectName);
    });
    
    await page.click('text=New Project');

    await expect(page).toHaveURL(new RegExp(`/${newProjectName}`));
    await request.post(`/api/delete/${encodeURIComponent(newProjectName)}`, {
      headers: { 'X-EasyTex-Request': 'true' }
    });
  });
});
