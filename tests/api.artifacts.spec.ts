import { expect, test } from '@playwright/test';
import { csrfHeaders, withProject } from './e2e-helpers';

test.describe('API artifacts', () => {
  test('reports no PDF before a build and validates unsafe run ids', async ({ request }) => {
    await withProject(request, async (project) => {
      const preview = await request.get(`/api/preview/${encodeURIComponent(project)}`);
      expect(preview.status()).toBe(404);

      const pdf = await request.get(`/pdf/${encodeURIComponent(project)}`);
      expect(pdf.status()).toBe(404);

      const badRun = await request.get(`/pdf/${encodeURIComponent(project)}?dl=1&run=..%2Fevil_S`);
      expect(badRun.status()).toBe(404);

      const builds = await request.get(`/api/builds/${encodeURIComponent(project)}`);
      expect(builds.status()).toBe(200);
      expect(await builds.json()).toEqual([]);

      const clean = await request.post(`/api/clean/${encodeURIComponent(project)}`, {
        headers: csrfHeaders(),
      });
      expect(clean.status()).toBe(200);
    }, 'api-artifacts');
  });
});
