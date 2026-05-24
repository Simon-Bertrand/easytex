import { expect, test } from '@playwright/test';
import { csrfHeaders, uniqueProjectName } from './e2e-helpers';

test.describe('API projects', () => {
  test('lists, creates, and deletes an isolated project', async ({ request }) => {
    const name = uniqueProjectName('api-project');

    const create = await request.post(`/api/create/${encodeURIComponent(name)}`, {
      headers: csrfHeaders(),
    });
    expect(create.status(), await create.text()).toBe(200);

    const projects = await request.get('/api/projects');
    expect(projects.status()).toBe(200);
    await expect.poll(async () => ((await (await request.get('/api/projects')).json()).projects)).toContain(name);

    const cfg = await request.get(`/api/config/${encodeURIComponent(name)}`);
    expect(cfg.status()).toBe(200);
    await expect(cfg.json()).resolves.toMatchObject({
      raw: expect.stringContaining('entrypoint = "main.tex"'),
    });

    const remove = await request.post(`/api/delete/${encodeURIComponent(name)}`, {
      headers: csrfHeaders(),
    });
    expect(remove.status(), await remove.text()).toBe(200);

    const after = await request.get('/api/projects');
    expect((await after.json()).projects).not.toContain(name);
  });

  test('rejects invalid project names', async ({ request }) => {
    for (const name of ['../x', 'bad name', 'x.y', 'x/y', '<script>']) {
      const response = await request.post(`/api/create/${encodeURIComponent(name)}`, {
        headers: csrfHeaders(),
      });
      expect(response.status(), name).toBe(400);
    }
  });
});
