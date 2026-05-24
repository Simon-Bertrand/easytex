import { expect, type APIRequestContext, type Page } from '@playwright/test';

export const csrfHeaders = () => {
  const headers: Record<string, string> = {
    'X-EasyTex-Request': 'true',
  };
  const token = process.env.EASYTEX_E2E_ADMIN_TOKEN;
  if (token) {
    headers.Authorization = `Bearer ${token}`;
  }
  return headers;
};

export const uniqueProjectName = (prefix = 'e2e') =>
  `${prefix}-${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;

export async function createProject(request: APIRequestContext, name = uniqueProjectName()) {
  const response = await request.post(`/api/create/${encodeURIComponent(name)}`, {
    headers: csrfHeaders(),
  });
  expect(response.status(), await response.text()).toBe(200);
  return name;
}

export async function deleteProject(request: APIRequestContext, name: string) {
  await request.post(`/api/delete/${encodeURIComponent(name)}`, {
    headers: csrfHeaders(),
  });
}

export async function withProject<T>(
  request: APIRequestContext,
  run: (name: string) => Promise<T>,
  prefix = 'e2e',
) {
  const name = await createProject(request, uniqueProjectName(prefix));
  try {
    return await run(name);
  } finally {
    await deleteProject(request, name);
  }
}

export async function saveProjectFile(
  request: APIRequestContext,
  project: string,
  path: string,
  content: string,
) {
  const response = await request.post(`/api/file/${encodeURIComponent(project)}`, {
    headers: csrfHeaders(),
    data: { path, content },
  });
  expect(response.status(), await response.text()).toBe(200);
}

export async function saveConfig(request: APIRequestContext, project: string, raw: string) {
  const response = await request.post(`/api/config/${encodeURIComponent(project)}`, {
    headers: csrfHeaders(),
    data: { raw },
  });
  expect(response.status(), await response.text()).toBe(200);
}

export async function openProject(page: Page, project: string) {
  await page.goto(`/${project}`);
  await expect(page.locator('header')).toContainText(project, { timeout: 15000 });
}

export async function installUiToken(page: Page) {
  const token = process.env.EASYTEX_E2E_ADMIN_TOKEN;
  if (!token) return;
  await page.addInitScript((value) => {
    window.localStorage.setItem('easytex_admin_token', value);
  }, token);
}
