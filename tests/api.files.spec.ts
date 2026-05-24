import { expect, test } from '@playwright/test';
import { csrfHeaders, saveProjectFile, withProject } from './e2e-helpers';

test.describe('API files', () => {
  test('lists, reads, and writes editable project files', async ({ request }) => {
    await withProject(request, async (project) => {
      await saveProjectFile(request, project, 'chapters/intro.tex', 'Hello from intro.');

      const list = await request.get(`/api/files/${encodeURIComponent(project)}`);
      expect(list.status()).toBe(200);
      const body = await list.json();
      expect(body.files).toEqual(expect.arrayContaining(['main.tex', 'EasyTex.toml', 'chapters/intro.tex']));
      expect(body.complete).toBe(true);

      const file = await request.get(`/api/file/${encodeURIComponent(project)}?path=chapters%2Fintro.tex`);
      expect(file.status()).toBe(200);
      await expect(file.json()).resolves.toMatchObject({
        path: 'chapters/intro.tex',
        content: 'Hello from intro.',
      });
    }, 'api-files');
  });

  test('rejects unsafe or non-editable file paths', async ({ request }) => {
    await withProject(request, async (project) => {
      const paths = ['../main.tex', '/etc/passwd', '.env', 'build/generated.tex', 'secret.bin', 'chapters\\intro.tex'];
      for (const path of paths) {
        const read = await request.get(`/api/file/${encodeURIComponent(project)}?path=${encodeURIComponent(path)}`);
        expect(read.status(), `read ${path}`).toBe(403);

        const write = await request.post(`/api/file/${encodeURIComponent(project)}`, {
          headers: csrfHeaders(),
          data: { path, content: 'x' },
        });
        expect(write.status(), `write ${path}`).toBe(403);
      }
    }, 'api-paths');
  });

  test('enforces CSRF on file writes', async ({ request }) => {
    await withProject(request, async (project) => {
      const response = await request.post(`/api/file/${encodeURIComponent(project)}`, {
        data: { path: 'main.tex', content: 'x' },
      });
      expect(response.status()).toBe(403);
    }, 'api-csrf');
  });
});
