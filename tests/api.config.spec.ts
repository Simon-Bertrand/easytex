import { expect, test } from '@playwright/test';
import { csrfHeaders, saveConfig, withProject } from './e2e-helpers';

test.describe('API config', () => {
  test('accepts valid config and rejects invalid TOML, entrypoints, and commands', async ({ request }) => {
    await withProject(request, async (project) => {
      await saveConfig(request, project, 'entrypoint = "main.tex"\nformat_command = "tex-fmt {file}"\n');

      const cases = [
        { raw: 'entrypoint = "', status: 400 },
        { raw: 'entrypoint = "../main.tex"', status: 400 },
        { raw: 'entrypoint = "main.tex; rm -rf /"', status: 400 },
        { raw: 'entrypoint = "main.tex"\nformat_command = "sh -c evil"', status: 400 },
        { raw: 'entrypoint = "main.tex"\n' + '#'.repeat(20_000), status: 413 },
      ];

      for (const item of cases) {
        const response = await request.post(`/api/config/${encodeURIComponent(project)}`, {
          headers: csrfHeaders(),
          data: { raw: item.raw },
        });
        expect(response.status(), item.raw).toBe(item.status);
      }
    }, 'api-config');
  });

  test('does not create a project implicitly when saving config', async ({ request }) => {
    const project = `missing-config-${Date.now()}`;
    const response = await request.post(`/api/config/${encodeURIComponent(project)}`, {
      headers: csrfHeaders(),
      data: { raw: 'entrypoint = "main.tex"\n' },
    });

    expect(response.status(), await response.text()).toBe(404);

    const projects = await request.get('/api/projects');
    expect((await projects.json()).projects).not.toContain(project);
  });
});
