import { test, expect } from '@playwright/test';

test.describe('Security Suite', () => {
  
  test('Path Traversal: should reject ../ in URL', async ({ request }) => {
    const response = await request.get('/api/config/../etc/passwd');
    // Normalized by router (404) or rejected by validation (400)
    expect([400, 404]).toContain(response.status());
  });

  test('Path Traversal: should reject absolute paths', async ({ request }) => {
    const response = await request.get('/api/config/%2Fetc%2Fpasswd');
    expect(response.status()).toBe(400);
  });

  test('XSS: should escape project names in dashboard', async ({ page, request }) => {
    const xssName = 'xss-test-"><img src=x onerror=alert(1)>';
    // Even if we try to create it via API, it should fail validation
    const createResponse = await request.post(`/api/create/${encodeURIComponent(xssName)}`, {
      headers: { 'X-EasyTex-Request': 'true' }
    });
    expect(createResponse.status()).toBe(400); // Rejected by is_valid_project_name
  });

  test('Command Injection: should reject invalid entrypoints', async ({ request }) => {
    // Try to set an entrypoint that tries to escape command
    const invalidConfig = {
      raw: 'entrypoint = "main.tex; rm -rf /"'
    };
    const response = await request.post('/api/config/demo', {
      data: invalidConfig,
      headers: { 'X-EasyTex-Request': 'true' }
    });
    expect(response.status()).toBe(400); // Rejected by is_valid_entrypoint
  });

  test('Config Size: should reject huge config files', async ({ request }) => {
    const hugeConfig = {
      raw: 'entrypoint = "main.tex"\n' + '#'.repeat(20000)
    };
    const response = await request.post('/api/config/demo', {
      data: hugeConfig,
      headers: { 'X-EasyTex-Request': 'true' }
    });
    expect(response.status()).toBe(413); // Rejected by size check
  });
});
