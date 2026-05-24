import { expect, test } from '@playwright/test';
import { csrfHeaders } from './e2e-helpers';

test.describe('admin and auth controls', () => {
  test('admin kill requires CSRF protection', async ({ request }) => {
    const response = await request.post('/api/admin/kill/demo');
    expect(response.status()).toBe(process.env.EASYTEX_E2E_ADMIN_TOKEN ? 401 : 403);
  });

  test('admin kill accepts CSRF header on local unprotected server', async ({ request }) => {
    test.skip(!!process.env.EASYTEX_E2E_ADMIN_TOKEN, 'Token-protected mode uses bearer-specific assertions.');
    const response = await request.post('/api/admin/kill/demo', {
      headers: csrfHeaders(),
    });
    expect(response.status()).toBe(200);
  });

  test('admin metrics returns operational counters', async ({ request }) => {
    test.skip(!!process.env.EASYTEX_E2E_ADMIN_TOKEN, 'Token-protected mode uses bearer-specific assertions.');
    const response = await request.get('/api/admin/metrics');
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.projects).toBeGreaterThanOrEqual(1);
    expect(body.max_concurrent_builds).toBeGreaterThan(0);
    expect(body.auth_required).toBe(false);
  });

  test('token-protected mode rejects unauthenticated read endpoints', async ({ request }) => {
    test.skip(!process.env.EASYTEX_E2E_ADMIN_TOKEN, 'Run with EASYTEX_ADMIN_TOKEN/EASYTEX_E2E_ADMIN_TOKEN to enable.');
    for (const url of ['/api/projects', '/api/config/demo', '/api/files/demo', '/pdf/demo', '/events/demo']) {
      const response = await request.get(url);
      expect(response.status(), url).toBe(401);
    }
  });

  test('token-protected mode accepts authenticated reads and admin metrics', async ({ request }) => {
    test.skip(!process.env.EASYTEX_E2E_ADMIN_TOKEN, 'Run with EASYTEX_ADMIN_TOKEN/EASYTEX_E2E_ADMIN_TOKEN to enable.');
    const projects = await request.get('/api/projects', { headers: csrfHeaders() });
    expect(projects.status()).toBe(200);

    const metrics = await request.get('/api/admin/metrics', { headers: csrfHeaders() });
    expect(metrics.status()).toBe(200);
    await expect(metrics.json()).resolves.toMatchObject({ auth_required: true });
  });

  test('mutating API rejects missing bearer in token-protected mode', async ({ request }) => {
    test.skip(!process.env.EASYTEX_E2E_ADMIN_TOKEN, 'Run with EASYTEX_ADMIN_TOKEN/EASYTEX_E2E_ADMIN_TOKEN to enable.');
    const response = await request.post('/api/create/token-missing', {
      headers: { 'X-EasyTex-Request': 'true' },
    });
    expect(response.status()).toBe(401);
  });

  test('mutating API accepts bearer in token-protected mode', async ({ request }) => {
    test.skip(!process.env.EASYTEX_E2E_ADMIN_TOKEN, 'Run with EASYTEX_ADMIN_TOKEN/EASYTEX_E2E_ADMIN_TOKEN to enable.');
    const project = `token-ok-${Date.now()}`;
    const create = await request.post(`/api/create/${project}`, {
      headers: csrfHeaders(),
    });
    expect(create.status(), await create.text()).toBe(200);

    const remove = await request.post(`/api/delete/${project}`, {
      headers: csrfHeaders(),
    });
    expect(remove.status(), await remove.text()).toBe(200);
  });
});
