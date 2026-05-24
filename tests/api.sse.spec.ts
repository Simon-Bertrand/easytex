import { expect, test } from '@playwright/test';
import { saveProjectFile, withProject } from './e2e-helpers';

test.describe('API server-sent events', () => {
  test('emits file_changed events for project edits', async ({ page, request }) => {
    await withProject(request, async (project) => {
      await page.goto('/');

      await page.evaluate((name) => {
        const state = window as typeof window & {
          __easytexEvents?: unknown[];
          __easytexSource?: EventSource;
        };
        state.__easytexEvents = [];
        const source = new EventSource(`/events/${encodeURIComponent(name)}`);
        state.__easytexSource = source;
        source.onmessage = (message) => {
          const event = JSON.parse(message.data);
          state.__easytexEvents?.push(event);
        };
      }, project);

      await expect
        .poll(async () => page.evaluate(() => {
          const state = window as typeof window & { __easytexSource?: EventSource };
          return state.__easytexSource?.readyState;
        }))
        .toBe(1);

      await saveProjectFile(request, project, 'main.tex', '\\documentclass{article}\\begin{document}Changed\\end{document}');
      await expect
        .poll(async () => page.evaluate(() => {
          const state = window as typeof window & { __easytexEvents?: Array<{ type: string; data: { path?: string } }> };
          return state.__easytexEvents?.find((event) => event.type === 'file_changed')?.data.path;
        }), { timeout: 20_000 })
        .toBe('main.tex');

      await page.evaluate(() => {
        const state = window as typeof window & { __easytexSource?: EventSource };
        state.__easytexSource?.close();
      });
    }, 'api-sse');
  });
});
