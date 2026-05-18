import type { BuildArtifactResponse } from './bindings/BuildArtifactResponse';
import type { ConfigResponse } from './bindings/ConfigResponse';
import type { FileListResponse } from './bindings/FileListResponse';
import type { FileResponse } from './bindings/FileResponse';
import type { LintResponse } from './bindings/LintResponse';
import type { PreviewResponse } from './bindings/PreviewResponse';
import type { ProjectsResponse } from './bindings/ProjectsResponse';
import type { ProjectStatusResponse } from './bindings/ProjectStatusResponse';
import type { RuntimeCapabilities } from './bindings/RuntimeCapabilities';
import type { ServerEvent } from './bindings/ServerEvent';
import type { SynctexEditResponse } from './bindings/SynctexEditResponse';
import type { SynctexViewResponse } from './bindings/SynctexViewResponse';

export type {
  BuildArtifactResponse,
  ConfigResponse,
  FileListResponse,
  FileResponse,
  LintResponse,
  PreviewResponse,
  ProjectsResponse,
  ProjectStatusResponse,
  RuntimeCapabilities,
  ServerEvent,
  SynctexEditResponse,
  SynctexViewResponse,
};

type EventHandlers = {
  onOpen?: () => void;
  onError?: () => void;
  onEvent: (event: ServerEvent) => void;
};

type JsonBody = Record<string, unknown>;

export class EasyTexClientError extends Error {
  status: number;
  body: string;

  constructor(status: number, body: string) {
    super(body || `EasyTex request failed with status ${status}`);
    this.name = 'EasyTexClientError';
    this.status = status;
    this.body = body;
  }
}

export class EasyTexClient {
  async capabilities(): Promise<RuntimeCapabilities> {
    return this.getJson('/api/capabilities');
  }

  async projects(): Promise<ProjectsResponse> {
    return this.getJson('/api/projects');
  }

  async projectStatus(project: string): Promise<ProjectStatusResponse> {
    return this.getJson(`/api/status/${encodeURIComponent(project)}`);
  }

  async config(project: string): Promise<ConfigResponse> {
    return this.getJson(`/api/config/${encodeURIComponent(project)}`);
  }

  async saveConfig(project: string, raw: string): Promise<void> {
    await this.postJson(`/api/config/${encodeURIComponent(project)}`, { raw });
  }

  async createProject(project: string): Promise<void> {
    await this.post(`/api/create/${encodeURIComponent(project)}`);
  }

  async deleteProject(project: string): Promise<void> {
    await this.post(`/api/delete/${encodeURIComponent(project)}`);
  }

  async projectFiles(project: string): Promise<FileListResponse> {
    return this.getJson(`/api/files/${encodeURIComponent(project)}`);
  }

  async projectFile(project: string, path: string): Promise<FileResponse> {
    const query = new URLSearchParams({ path });
    return this.getJson(`/api/file/${encodeURIComponent(project)}?${query}`);
  }

  async saveProjectFile(project: string, path: string, content: string): Promise<void> {
    await this.postJson(`/api/file/${encodeURIComponent(project)}`, { content, path });
  }

  async run(project: string): Promise<void> {
    await this.post(`/api/run/${encodeURIComponent(project)}`);
  }

  async cancel(project: string): Promise<void> {
    await this.post(`/api/cancel/${encodeURIComponent(project)}`);
  }

  async format(project: string): Promise<void> {
    await this.post(`/api/format/${encodeURIComponent(project)}`);
  }

  async clean(project: string): Promise<void> {
    await this.post(`/api/clean/${encodeURIComponent(project)}`);
  }

  async lint(project: string): Promise<LintResponse> {
    return this.postJson(`/api/lint/${encodeURIComponent(project)}`);
  }

  async preview(project: string): Promise<PreviewResponse> {
    return this.getJson(`/api/preview/${encodeURIComponent(project)}?t=${Date.now()}`);
  }

  async builds(project: string): Promise<BuildArtifactResponse[]> {
    return this.getJson(`/api/builds/${encodeURIComponent(project)}?t=${Date.now()}`);
  }

  async synctexEdit(
    project: string,
    page: number,
    x: number,
    y: number,
  ): Promise<SynctexEditResponse> {
    const query = new URLSearchParams({
      mode: 'edit',
      page: page.toString(),
      x: x.toFixed(1),
      y: y.toFixed(1),
    });
    return this.getJson(`/api/synctex/${encodeURIComponent(project)}?${query}`);
  }

  async synctexView(
    project: string,
    line: number,
    col: number,
    path: string,
  ): Promise<SynctexViewResponse> {
    const query = new URLSearchParams({
      mode: 'view',
      line: line.toString(),
      col: col.toString(),
      path,
    });
    return this.getJson(`/api/synctex/${encodeURIComponent(project)}?${query}`);
  }

  pdfUrl(project: string): string {
    return `/pdf/${encodeURIComponent(project)}?t=${Date.now()}`;
  }

  pdfDownloadUrl(project: string, run?: string): string {
    const query = new URLSearchParams({ dl: '1' });
    if (run) query.set('run', run);
    return `/pdf/${encodeURIComponent(project)}?${query}`;
  }

  connectEvents(project: string, handlers: EventHandlers): () => void {
    const es = new EventSource(`/events/${encodeURIComponent(project)}`);
    es.onopen = () => handlers.onOpen?.();
    es.onerror = () => handlers.onError?.();
    es.onmessage = ({ data }) => {
      handlers.onEvent(JSON.parse(data) as ServerEvent);
    };
    return () => es.close();
  }

  private async getJson<T>(url: string): Promise<T> {
    return this.requestJson(url, { method: 'GET' });
  }

  private async post(url: string): Promise<void> {
    await this.request(url, { method: 'POST' });
  }

  private async postJson<T = void>(url: string, body?: JsonBody): Promise<T> {
    return this.requestJson(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: body ? JSON.stringify(body) : undefined,
    });
  }

  private async requestJson<T>(url: string, init: RequestInit): Promise<T> {
    const res = await this.request(url, init);
    return await res.json() as T;
  }

  private async request(url: string, init: RequestInit): Promise<Response> {
    const headers = new Headers(init.headers || {});
    headers.set('X-EasyTex-Request', 'true');
    const res = await fetch(url, { ...init, headers });
    if (!res.ok) {
      throw new EasyTexClientError(res.status, await res.text());
    }
    return res;
  }
}

export const easytexClient = new EasyTexClient();
