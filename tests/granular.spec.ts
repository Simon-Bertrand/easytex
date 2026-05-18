import { test, expect } from '@playwright/test';

test.describe('EasyTex Granular E2E Suite', () => {
  const connected = /Connecté au serveur|Connected to server/;

  test.beforeEach(async ({ page }) => {
    await page.goto('/');
  });

  test('Path Traversal: should reject ../ in URL', async ({ request }) => {
    const response = await request.get('/api/config/../etc/passwd');
    // Normalized by router or rejected by validation
    expect([400, 404]).toContain(response.status());
  });

  test('Dashboard: Project List Rendering', async ({ page }) => {
    const projectCard = page.locator('.project-card', { hasText: 'demo' });
    await expect(projectCard).toBeVisible();
  });

  test('Navigation: Open Project', async ({ page }) => {
    const card = page.locator('.project-card', { hasText: 'demo' });
    await expect(card).toBeVisible({ timeout: 10000 });
    await card.click();
    await expect(page).toHaveURL(/\/demo/);
    await expect(page.locator('header')).toContainText('demo');
  });

  test('Project: SSE Connection', async ({ page }) => {
    await page.goto('/demo');
    const logContainer = page.locator('#l');
    await expect(logContainer).toContainText(connected, { timeout: 30000 });
  });

  test('Project: Build & Tools Cycle', async ({ page }) => {
    await page.goto('/demo');
    await expect(page.locator('#l')).toContainText(connected, { timeout: 30000 });

    const btnRun = page.locator('#btn-run');
    const btnFormat = page.locator('button:has-text("Format")');
    const btnClean = page.locator('button:has-text("Clean")');
    
    // Wait for initial build/PDF to be ready
    await expect(page.locator('canvas').first()).toBeVisible({ timeout: 45000 });
    await expect(btnRun).toContainText('Run', { timeout: 45000 });
    
    // 1. Test Run
    await btnRun.click();
    await expect(btnRun).toContainText('Stop', { timeout: 15000 });
    await expect(btnRun).toContainText('Run', { timeout: 45000 });

    // 2. Test Format
    await btnFormat.click();
    await expect(page.locator('#l')).toContainText(/Format/i, { timeout: 15000 });

    // 3. Test Clean
    await btnClean.click();
    await expect(page.locator('#l')).toContainText(/Clean/i, { timeout: 15000 });

    // Restore compiled PDF for subsequent tests
    await btnRun.click();
    await expect(btnRun).toContainText('Stop', { timeout: 15000 });
    await expect(btnRun).toContainText('Run', { timeout: 45000 });
  });

  test('Project: PDF Viewing & Zoom', async ({ page }) => {
    await page.goto('/demo');
    await expect(page.locator('#l')).toContainText(connected, { timeout: 30000 });
    
    // Wait for PDF render
    const canvas = page.locator('canvas').first();
    await expect(canvas).toBeVisible({ timeout: 45000 });
    
    const zoomVal = page.locator('#zoom-val');
    const pgNav = page.locator('#pg-nav');
    await expect(pgNav).toBeVisible({ timeout: 10000 });
    
    const initialZoom = await zoomVal.innerText();
    await page.click('.pg-btn:has-text("+")');
    await expect(zoomVal).not.toHaveText(initialZoom);
  });

  test('Project: Configuration Modal', async ({ page }) => {
    await page.goto('/demo');
    await expect(page.locator('#l')).toContainText(connected, { timeout: 30000 });
    await page.click('button:has-text("Settings")');
    
    const modal = page.locator('#modal');
    await expect(modal).toBeVisible();
    
    const textarea = page.locator('#cfg-txt');
    await expect(textarea).not.toBeEmpty();
  });

  test('Flow: Create New Project', async ({ page, request }) => {
    const newName = `test-${Date.now()}`;
    
    page.on('dialog', async dialog => {
      await dialog.accept(newName);
    });
    
    const btnNew = page.locator('button:has-text("New Project")');
    await expect(btnNew).toBeVisible();
    await btnNew.click();
    
    await expect(page).toHaveURL(new RegExp(`/${newName}`));
    await expect(page.locator('header')).toContainText(newName);
    await request.post(`/api/delete/${encodeURIComponent(newName)}`, {
      headers: { 'X-EasyTex-Request': 'true' }
    });
  });

});
