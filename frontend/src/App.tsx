import { createSignal, createEffect, onMount, For, Show, onCleanup } from 'solid-js';
import type { PDFDocumentProxy } from 'pdfjs-dist';
import { 
  Play, 
  Square, 
  ChevronLeft, 
  ChevronRight, 
  Plus, 
  Layout, 
  Download,
} from 'lucide-solid';

import { EditorView, basicSetup } from 'codemirror';
import { EditorState } from '@codemirror/state';
import { keymap } from '@codemirror/view';
import { indentWithTab } from '@codemirror/commands';
import { StreamLanguage } from '@codemirror/language';
import { stex } from '@codemirror/legacy-modes/mode/stex';
import type { DiagnosticEvent } from './bindings/DiagnosticEvent';
import {
  EasyTexClientError,
  easytexClient,
  type BuildArtifactResponse,
  type LintResponse,
  type PreviewResponse,
  type RuntimeCapabilities,
  type ServerEvent,
} from './client';

let pdfjsModule: typeof import('pdfjs-dist') | null = null;

const loadPdfjs = async () => {
  if (!pdfjsModule) {
    const [pdfjs, worker] = await Promise.all([
      import('pdfjs-dist'),
      import('pdfjs-dist/build/pdf.worker.mjs?url'),
    ]);
    pdfjs.GlobalWorkerOptions.workerSrc = worker.default;
    pdfjsModule = pdfjs;
  }
  return pdfjsModule;
};

interface LogEntry {
  msg: string;
  lvl: string;
  id: number;
}

type ProjectStats = Extract<ServerEvent, { type: 'stats' }>['data'];
type BuildDiagnostic = DiagnosticEvent & { id: number };

interface TreeNode {
  name: string;
  path: string;
  isDir: boolean;
  children?: TreeNode[];
}

const buildFileTree = (filePaths: string[]): TreeNode[] => {
  const rootNodes: TreeNode[] = [];

  for (const path of filePaths) {
    const parts = path.split('/');
    let currentLevel = rootNodes;
    let accumulatedPath = '';

    for (let i = 0; i < parts.length; i++) {
      const part = parts[i];
      accumulatedPath = accumulatedPath ? `${accumulatedPath}/${part}` : part;
      const isLast = i === parts.length - 1;

      let existingNode = currentLevel.find(n => n.name === part);
      if (!existingNode) {
        existingNode = {
          name: part,
          path: accumulatedPath,
          isDir: !isLast,
          children: isLast ? undefined : []
        };
        currentLevel.push(existingNode);
      }
      if (!isLast && existingNode.children) {
        currentLevel = existingNode.children;
      }
    }
  }

  const sortTree = (nodes: TreeNode[]) => {
    nodes.sort((a, b) => {
      if (a.isDir && !b.isDir) return -1;
      if (!a.isDir && b.isDir) return 1;
      return a.name.localeCompare(b.name);
    });
    for (const node of nodes) {
      if (node.children) {
        sortTree(node.children);
      }
    }
  };

  sortTree(rootNodes);
  return rootNodes;
};

// Custom Premium Dark Slate Theme for CodeMirror
const customTheme = EditorView.theme({
  "&": {
    color: "#e6e9ef",
    backgroundColor: "#111317",
    height: "100%"
  },
  ".cm-content": {
    caretColor: "#38bdf8",
    padding: "16px 0"
  },
  "&.cm-focused .cm-cursor": {
    borderLeftColor: "#38bdf8"
  },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": {
    backgroundColor: "rgba(56, 189, 248, 0.2) !important"
  },
  ".cm-gutters": {
    backgroundColor: "#111317",
    color: "#4e5767",
    border: "none",
    paddingLeft: "8px"
  },
  ".cm-activeLine": {
    backgroundColor: "rgba(255,255,255,0.02)"
  },
  ".cm-activeLineGutter": {
    backgroundColor: "rgba(255,255,255,0.02)",
    color: "#e6e9ef"
  }
}, { dark: true });

function App() {
  const [projectName, setProjectName] = createSignal(window.location.pathname.split('/')[1] || '');
  const [projects, setProjects] = createSignal<string[]>([]);
  const [logs, setLogs] = createSignal<LogEntry[]>([]);
  const [stats, setStats] = createSignal<ProjectStats | null>(null);
  const [previewInfo, setPreviewInfo] = createSignal<PreviewResponse | null>(null);
  const [successBuilds, setSuccessBuilds] = createSignal<BuildArtifactResponse[]>([]);
  const [nowMs, setNowMs] = createSignal(Date.now());
  const [diagnostics, setDiagnostics] = createSignal<BuildDiagnostic[]>([]);
  const [isBuilding, setIsBuilding] = createSignal(false);
  const [isConnected, setIsConnected] = createSignal(false);
  const [capabilities, setCapabilities] = createSignal<RuntimeCapabilities>({
    format: false,
    lint: false,
    synctex: false,
    pdf_compression: false,
    log_analysis: false,
    read_only: false
  });
  const [showErrorModal, setShowErrorModal] = createSignal(false);
  const [showLintModal, setShowLintModal] = createSignal(false);
  const [lintResult, setLintResult] = createSignal<LintResponse | null>(null);
  const [isLinting, setIsLinting] = createSignal(false);
  const [showDownloadsModal, setShowDownloadsModal] = createSignal(false);
  const [showExportMenu, setShowExportMenu] = createSignal(false);
  const [showConfigModal, setShowConfigModal] = createSignal(false);
  const [configRaw, setConfigRaw] = createSignal<string>('');
  const [isSavingConfig, setIsSavingConfig] = createSignal(false);
  const [configError, setConfigError] = createSignal<string | null>(null);
  const [showAuthModal, setShowAuthModal] = createSignal(false);
  const [authTokenInput, setAuthTokenInput] = createSignal(window.localStorage.getItem('easytex_admin_token') || '');

  const errorLogs = () => logs().filter(l => l.lvl === 'err');
  const warningLogs = () => logs().filter(l => l.lvl === 'warn');
  const errorDiagnostics = () => diagnostics().filter(d => d.severity === 'err');
  const warningDiagnostics = () => diagnostics().filter(d => d.severity === 'warn');
  const errorCount = () => errorDiagnostics().length || errorLogs().length;
  const isReadOnly = () => capabilities().read_only;
  const previewAge = () => {
    const info = previewInfo();
    if (!info?.built_at_ms) return null;
    const seconds = Math.max(0, Math.floor((nowMs() - info.built_at_ms) / 1000));
    if (seconds < 60) return `Built ${seconds}s ago`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `Built ${minutes} min ago`;
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return `Built ${hours}h ago`;
    const days = Math.floor(hours / 24);
    return `Built ${days}d ago`;
  };
  const formatBuildAge = (builtAtMs: number) => {
    if (!builtAtMs) return 'Unknown date';
    const seconds = Math.max(0, Math.floor((nowMs() - builtAtMs) / 1000));
    if (seconds < 60) return `${seconds}s ago`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `${minutes} min ago`;
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return `${hours}h ago`;
    return `${Math.floor(hours / 24)}d ago`;
  };
  const formatBytes = (bytes: number) => {
    if (bytes > 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
    if (bytes > 1024) return `${Math.round(bytes / 1024)} KB`;
    return `${bytes} B`;
  };

  const [zoom, setZoom] = createSignal(1.0);
  const [curPage, setCurPage] = createSignal(1);
  const [totalPage, setTotalPage] = createSignal(1);
  
  // File Explorer signals
  const [files, setFiles] = createSignal<string[]>([]);
  const [activeFile, setActiveFile] = createSignal<string>('');
  const [showEditor, setShowEditor] = createSignal(true);
  const [expandedFolders, setExpandedFolders] = createSignal<Set<string>>(new Set<string>());

  const toggleFolder = (path: string) => {
    const next = new Set(expandedFolders());
    if (next.has(path)) {
      next.delete(path);
    } else {
      next.add(path);
    }
    setExpandedFolders(next);
  };

  // Automatically expand parent folders of active file when it changes
  createEffect(() => {
    const active = activeFile();
    if (!active) return;
    const parts = active.split('/');
    if (parts.length > 1) {
      const next = new Set(expandedFolders());
      let accumulated = '';
      for (let i = 0; i < parts.length - 1; i++) {
        accumulated = accumulated ? `${accumulated}/${parts[i]}` : parts[i];
        next.add(accumulated);
      }
      setExpandedFolders(next);
    }
  });

  // Automatically scroll console to bottom when logs update
  createEffect(() => {
    logs();
    setTimeout(() => {
      if (logContainer && !userScrolledUp) {
        logContainer.scrollTop = logContainer.scrollHeight;
      }
    }, 40);
  });

  const FileTreeNode = (props: { node: TreeNode, depth: number }) => {
    const isExpanded = () => expandedFolders().has(props.node.path);

    if (props.node.isDir) {
      return (
        <div style={{ "margin-left": `${props.depth === 0 ? 0 : 6}px`, "display": "flex", "flex-direction": "column", "gap": "2px" }}>
          <div 
            class="file-item folder-item"
            onClick={() => toggleFolder(props.node.path)}
            style={{ 
              "display": "flex", 
              "align-items": "center", 
              "gap": "8px", 
              "cursor": "pointer", 
              "padding": "6px 8px 6px 12px", 
              "border-radius": "4px",
              "user-select": "none"
            }}
          >
            <span style={{ 
              "color": "var(--sub)", 
              "font-size": "8px", 
              "transition": "transform 0.15s ease", 
              "transform": isExpanded() ? "rotate(90deg)" : "rotate(0deg)", 
              "display": "inline-block",
              "width": "8px",
              "text-align": "center"
            }}>▶</span>
            <span style={{ "font-size": "14px" }}>📁</span>
            <span class="file-name" style={{ "color": "var(--txt)", "font-weight": "500", "font-size": "13px" }}>{props.node.name}</span>
          </div>
          <Show when={isExpanded()}>
            <div class="folder-children" style={{ "display": "flex", "flex-direction": "column", "gap": "2px", "padding-left": "8px", "border-left": "1px solid rgba(255,255,255,0.05)", "margin-left": "16px" }}>
              <For each={props.node.children}>
                {(child) => <FileTreeNode node={child} depth={props.depth + 1} />}
              </For>
            </div>
          </Show>
        </div>
      );
    } else {
      return (
        <div style={{ "margin-left": `${props.depth === 0 ? 0 : 0}px` }}>
          <div 
            class="file-item" 
            classList={{ active: activeFile() === props.node.path }}
            onClick={() => setActiveFile(props.node.path)}
            style={{ 
              "display": "flex", 
              "align-items": "center", 
              "gap": "10px", 
              "cursor": "pointer", 
              "padding": "6px 8px 6px 12px", 
              "border-radius": "4px"
            }}
          >
            <span style={{ "font-size": "14px" }}>📄</span>
            <span class="file-name" style={{ "font-size": "13px", "overflow": "hidden", "text-overflow": "ellipsis", "white-space": "nowrap" }}>{props.node.name}</span>
          </div>
        </div>
      );
    }
  };

  // Resizable layout signals
  const [leftWidth, setLeftWidth] = createSignal(55);
  const [topHeight, setTopHeight] = createSignal(500);
  const [sidebarWidth, setSidebarWidth] = createSignal(220);

  let logContainer: HTMLDivElement | undefined;
  let userScrolledUp = false;
  const handleLogScroll = () => {
    if (!logContainer) return;
    const atBottom = logContainer.scrollHeight - logContainer.scrollTop - logContainer.clientHeight <= 25;
    userScrolledUp = !atBottom;
  };
  let viewerContainer: HTMLDivElement | undefined;
  let editorContainer: HTMLDivElement | undefined;
  let pdfDoc: PDFDocumentProxy | null = null;
  let intersectionObserver: IntersectionObserver | null = null;
  let editorView: EditorView | null = null;
  let saveTimeout: any = null;

  // Sync project name with URL
  onMount(() => {
    window.onpopstate = () => {
      setProjectName(window.location.pathname.split('/')[1] || '');
    };
    
    const handleKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey) {
        if (e.key === '+' || e.key === '=') { e.preventDefault(); updateZoom(0.1); }
        else if (e.key === '-') { e.preventDefault(); updateZoom(-0.1); }
        else if (e.key === '0') { e.preventDefault(); updateZoom(0); }
        else if (e.key === 'b') { e.preventDefault(); runAction(); }
      }
    };
    window.addEventListener('keydown', handleKey);
    onCleanup(() => window.removeEventListener('keydown', handleKey));

    const handleAuthRequired = () => setShowAuthModal(true);
    window.addEventListener('easytex-auth-required', handleAuthRequired);
    onCleanup(() => window.removeEventListener('easytex-auth-required', handleAuthRequired));

    const clock = window.setInterval(() => setNowMs(Date.now()), 1000);
    onCleanup(() => window.clearInterval(clock));

  });

  createEffect(() => {
    if (!projectName()) {
      fetchProjects();
    }
  });

  onMount(() => {
    fetchCapabilities();
  });

  const fetchCapabilities = async () => {
    try {
      setCapabilities(await easytexClient.capabilities());
    } catch (e) {
      setCapabilities({
        format: false,
        lint: false,
        synctex: false,
        pdf_compression: false,
        log_analysis: false,
        read_only: false
      });
    }
  };

  const fetchProjects = async () => {
    try {
      const data = await easytexClient.projects();
      setProjects(data.projects);
    } catch (e) {
      console.error('Failed to fetch projects', e);
    }
  };

  const fetchProjectStatus = async (name: string) => {
    try {
      const data = await easytexClient.projectStatus(name);
      setIsBuilding(data.status === 'building');
    } catch (e) {
      console.error("Failed to fetch project status", e);
    }
  };

  const fetchConfig = async () => {
    const name = projectName();
    if (!name) return;
    setConfigError(null);
    try {
      const data = await easytexClient.config(name);
      setConfigRaw(data.raw);
      setShowConfigModal(true);
    } catch (e) {
      console.error("Failed to load config", e);
    }
  };

  const saveConfig = async () => {
    const name = projectName();
    if (!name) return;
    setIsSavingConfig(true);
    setConfigError(null);
    try {
      await easytexClient.saveConfig(name, configRaw());
      setShowConfigModal(false);
      await fetchProjectFiles(name);
      await loadPdf();
    } catch (e) {
      console.error("Failed to save config", e);
      let errMsg = "Failed to save configuration.";
      if (e instanceof EasyTexClientError) {
        try {
          const parsed = JSON.parse(e.body);
          if (parsed.message) {
            errMsg = parsed.message;
            if (errMsg.toLowerCase().includes("toml")) {
              errMsg = `TOML Error: ${errMsg}`;
            }
          } else {
            errMsg = e.body;
          }
        } catch {
          errMsg = e.body;
        }
      } else if (e instanceof Error) {
        errMsg = e.message;
      }
      setConfigError(errMsg);
    } finally {
      setIsSavingConfig(false);
    }
  };

  const navigateTo = (p: string) => {
    window.history.pushState({}, '', `/${p}`);
    setProjectName(p);
  };

  const deleteProject = async (p: string, e: Event) => {
    e.stopPropagation();
    if (!confirm(`Are you sure you want to delete project "${p}"?`)) return;
    try {
      await easytexClient.deleteProject(p);
      await fetchProjects();
    } catch (err) {
      console.error("Failed to delete project", err);
      alert("Failed to delete project.");
    }
  };

  const createProject = async () => {
    const n = prompt("Enter project name:");
    if (!n || !n.trim()) return;
    try {
      await easytexClient.createProject(n.trim());
      navigateTo(n.trim());
    } catch (e) {
      alert(e instanceof Error ? e.message : 'Failed to create project.');
    }
  };

  // Fetch file list when project changes
  createEffect(() => {
    const p = projectName();
    if (!p) {
      setFiles([]);
      setActiveFile('');
      if (editorView) {
        editorView.destroy();
        editorView = null;
      }
      return;
    }

    fetchProjectFiles(p);
    fetchProjectStatus(p);
  });

  const fetchProjectFiles = async (name: string) => {
    try {
      const data = await easytexClient.projectFiles(name);
      const list = data.files;
      setFiles(list);
      
      if (!activeFile() || !list.includes(activeFile())) {
        // Default to main.tex or first .tex file
        const entry = list.find((f: string) => f === 'main.tex' || f.endsWith('.tex')) || list[0] || '';
        setActiveFile(entry);
      }
    } catch (e) {
      console.error('Failed to fetch project files', e);
    }
  };

  // Load file content when active file changes
  createEffect(() => {
    const p = projectName();
    const f = activeFile();
    if (!p || !f) return;

    fetchFileContent(p, f);
  });

  const fetchFileContent = async (name: string, filePath: string, isReload: boolean = false) => {
    try {
      const data = await easytexClient.projectFile(name, filePath);
      if (isReload && editorView) {
        const currentVal = editorView.state.doc.toString();
        if (currentVal !== data.content) {
          const selection = editorView.state.selection;
          editorView.dispatch({
            changes: { from: 0, to: editorView.state.doc.length, insert: data.content },
            selection: selection
          });
        }
      } else {
        setupEditor(data.content);
      }
    } catch (e) {
      console.error("Failed to load file content", e);
    }
  };

  const setupEditor = (initialContent: string) => {
    if (!editorContainer) return;
    if (editorView) {
      editorView.destroy();
    }

    const state = EditorState.create({
      doc: initialContent,
      extensions: [
        basicSetup,
        StreamLanguage.define(stex),
        customTheme,
        keymap.of([
          indentWithTab,
          {
            key: "Mod-s",
            run: () => {
              if (isReadOnly()) return true;
              const val = editorView ? editorView.state.doc.toString() : '';
              saveFileContent(projectName(), val, true);
              return true;
            }
          },
          {
            key: "Mod-Enter",
            run: () => {
              syncForwardFromCursor();
              return true;
            }
          }
        ]),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            if (isReadOnly()) return;
            const val = update.state.doc.toString();
            triggerDebouncedSave(val);
          }
        }),
        EditorView.domEventHandlers({
          dblclick: (event, view) => {
            const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
            if (pos !== null) {
              const line = view.state.doc.lineAt(pos);
              syncForward(line.number, pos - line.from + 1);
            }
            return true;
          }
        })
      ]
    });

    editorView = new EditorView({
      state,
      parent: editorContainer
    });
  };

  const triggerDebouncedSave = (content: string) => {
    if (saveTimeout) clearTimeout(saveTimeout);
    saveTimeout = setTimeout(() => {
      saveFileContent(projectName(), content, false);
    }, 1500);
  };

  const saveFileContent = async (name: string, content: string, compileNow: boolean) => {
    try {
      await easytexClient.saveProjectFile(name, activeFile(), content);
      if (compileNow) await easytexClient.run(name);
    } catch (e) {
      console.error("Failed to save file", e);
    }
  };

  const runLint = async () => {
    const name = projectName();
    if (!name) return;
    setLintResult(null);
    setShowLintModal(true);
    setIsLinting(true);
    try {
      setLintResult(await easytexClient.lint(name));
    } catch (e) {
      setLintResult({
        ok: false,
        status: e instanceof EasyTexClientError ? e.status : null,
        stdout: '',
        stderr: e instanceof Error ? e.message : 'Failed to run chktex.'
      });
    } finally {
      setIsLinting(false);
    }
  };

  const createFile = async () => {
    const path = prompt("Enter the new file path (for example: chapters/conclusion.tex):");
    if (!path || !path.trim()) return;
    const trimmed = path.trim();
    
    try {
      await easytexClient.saveProjectFile(projectName(), trimmed, '');
      await fetchProjectFiles(projectName());
      setActiveFile(trimmed);
    } catch (e) {
      console.error("Failed to create file", e);
      alert("Failed to create the file.");
    }
  };

  const jumpToLine = (line: number, column: number) => {
    if (!editorView) return;
    try {
      const state = editorView.state;
      if (line > state.doc.lines) return;
      const lineObj = state.doc.line(line);
      const pos = Math.min(lineObj.from + (column - 1), lineObj.to);
      
      editorView.dispatch({
        selection: { anchor: pos, head: pos },
        scrollIntoView: true
      });
      
      editorView.focus();
    } catch (e) {
      console.error("Failed to jump to line in CodeMirror", e);
    }
  };

  const openDiagnostic = (diagnostic: BuildDiagnostic) => {
    if (diagnostic.file && diagnostic.file !== activeFile() && files().includes(diagnostic.file)) {
      setActiveFile(diagnostic.file);
      setTimeout(() => {
        if (diagnostic.line) jumpToLine(diagnostic.line, diagnostic.column || 1);
      }, 150);
      return;
    }
    if (diagnostic.line) {
      jumpToLine(diagnostic.line, diagnostic.column || 1);
    }
  };

  const initXResize = (e: MouseEvent) => {
    e.preventDefault();
    const doResize = (moveEvent: MouseEvent) => {
      const nextPercent = (moveEvent.clientX / document.body.clientWidth) * 100;
      setLeftWidth(Math.max(20, Math.min(80, nextPercent)));
      if (editorView) {
        editorView.requestMeasure();
      }
      window.dispatchEvent(new Event('resize'));
    };

    const stopResize = () => {
      document.removeEventListener('mousemove', doResize);
      document.removeEventListener('mouseup', stopResize);
    };

    document.addEventListener('mousemove', doResize);
    document.addEventListener('mouseup', stopResize);
  };

  const initYResize = (e: MouseEvent) => {
    e.preventDefault();
    const leftPaneTopEl = document.getElementById('left-pane-top');
    const initialHeight = leftPaneTopEl ? leftPaneTopEl.offsetHeight : 500;
    const startY = e.clientY;

    const doResize = (moveEvent: MouseEvent) => {
      const deltaY = moveEvent.clientY - startY;
      const nextHeight = Math.max(150, Math.min(1000, initialHeight + deltaY));
      setTopHeight(nextHeight);
      if (editorView) {
        editorView.requestMeasure();
      }
      window.dispatchEvent(new Event('resize'));
    };

    const stopResize = () => {
      document.removeEventListener('mousemove', doResize);
      document.removeEventListener('mouseup', stopResize);
    };

    document.addEventListener('mousemove', doResize);
    document.addEventListener('mouseup', stopResize);
  };

  const initSidebarResize = (e: MouseEvent) => {
    e.preventDefault();
    const leftPaneEl = document.getElementById('left-pane');
    if (!leftPaneEl) return;

    const doResize = (moveEvent: MouseEvent) => {
      const rect = leftPaneEl.getBoundingClientRect();
      const nextWidth = moveEvent.clientX - rect.left;
      setSidebarWidth(Math.max(120, Math.min(450, nextWidth)));
      if (editorView) {
        editorView.requestMeasure();
      }
      window.dispatchEvent(new Event('resize'));
    };

    const stopResize = () => {
      document.removeEventListener('mousemove', doResize);
      document.removeEventListener('mouseup', stopResize);
    };

    document.addEventListener('mousemove', doResize);
    document.addEventListener('mouseup', stopResize);
  };

  // EventSource Connection
  createEffect(() => {
    const p = projectName();
    if (!p) return;

    const closeEvents = easytexClient.connectEvents(p, {
      onOpen: () => {
        setIsConnected(true);
        setLogs([{ msg: 'Connected to server', lvl: 'ok', id: Date.now() }]);
        loadPdf();
      },
      onError: () => {
        setIsConnected(false);
        setLogs(prev => [...prev, { msg: 'Connection lost, reconnecting...', lvl: 'err', id: Date.now() }]);
        closeEvents();
        setTimeout(() => window.location.reload(), 2000);
      },
      onEvent: (m) => {
        if (m.type === 'log') {
          setLogs(prev => {
            const next = [...prev, { ...m.data, id: Date.now() }];
            return next.slice(-500);
          });
        } else if (m.type === 'diagnostic') {
          setDiagnostics(prev => {
            const next = [...prev, { ...m.data, id: Date.now() }];
            return next.slice(-200);
          });
        } else if (m.type === 'pdf') {
          setLogs(prev => [...prev, { msg: 'Reloading PDF...', lvl: 'info', id: Date.now() }]);
          loadPdf();
        } else if (m.type === 'stats') {
          setStats(m.data);
        } else if (m.type === 'status') {
          const building = m.data !== 'Idle' && m.data !== 'Error' && m.data !== 'TimedOut';
          if (building) setDiagnostics([]);
          setIsBuilding(building);
          document.title = building ? `🔨 ${projectName()}...` : `EasyTex — ${projectName()}`;
        } else if (m.type === 'file_changed') {
          const changedPath = m.data.path;
          if (changedPath === activeFile()) {
            fetchFileContent(projectName(), activeFile(), true);
          }
          fetchProjectFiles(projectName());
        }
      },
    });

    onCleanup(closeEvents);
  });

  const loadPdf = async () => {
    const p = projectName();
    if (!p) return;

    try {
      await loadPreviewInfo(p);
      const pdfjs = await loadPdfjs();
      const loadingTask = pdfjs.getDocument(easytexClient.pdfDocumentRequest(p));
      pdfDoc = await loadingTask.promise;
      setTotalPage(pdfDoc.numPages);
      renderAllPages();
    } catch (e) {
      console.warn('PDF Load failed', e);
    }
  };

  const loadPreviewInfo = async (name: string) => {
    try {
      setPreviewInfo(await easytexClient.preview(name));
    } catch (e) {
      setPreviewInfo(null);
    }
  };

  const openDownloads = async () => {
    const name = projectName();
    if (!name) return;
    try {
      setSuccessBuilds(await easytexClient.builds(name));
    } catch (e) {
      setSuccessBuilds([]);
    }
    setShowDownloadsModal(true);
  };

  const downloadBuild = (run: string) => {
    easytexClient.downloadPdf(projectName(), run).catch((error) => {
      console.error('PDF download failed', error);
    });
  };

  const downloadLatestPdf = () => {
    setShowExportMenu(false);
    easytexClient.downloadPdf(projectName()).catch((error) => {
      console.error('PDF download failed', error);
    });
  };

  const openDownloadsFromExport = async () => {
    setShowExportMenu(false);
    await openDownloads();
  };

  const renderAllPages = () => {
    if (!viewerContainer || !pdfDoc) return;
    
    viewerContainer.innerHTML = '';
    if (intersectionObserver) intersectionObserver.disconnect();

    intersectionObserver = new IntersectionObserver((entries) => {
      entries.forEach(entry => {
        if (entry.isIntersecting) {
          const pageNum = parseInt((entry.target as HTMLElement).dataset.n || '1');
          renderPage(pageNum, entry.target.querySelector('canvas') as HTMLCanvasElement);
        }
      });
    }, { root: viewerContainer, rootMargin: '600px' });

    for (let i = 1; i <= pdfDoc.numPages; i++) {
      const wrap = document.createElement('div');
      wrap.className = 'pg-wrap';
      wrap.dataset.n = i.toString();
      wrap.style.width = `min(100%, ${850 * zoom()}px)`;
      
      const canvas = document.createElement('canvas');
      wrap.appendChild(canvas);
      
      // Double Click on PDF Page triggers Inverse Search
      wrap.addEventListener('dblclick', (e) => handlePdfDblClick(e, i, canvas));
      
      viewerContainer.appendChild(wrap);
      intersectionObserver.observe(wrap);
    }
  };

  const renderPage = async (n: number, canvas: HTMLCanvasElement) => {
    if (!pdfDoc || !canvas || canvas.dataset.rendered === 'true') return;
    
    try {
      const page = await pdfDoc.getPage(n);
      const dpr = window.devicePixelRatio || 1;
      const viewport = page.getViewport({ scale: (dpr > 1 ? 1.5 : 2.0) * zoom() });
      
      // Store PDF page point boundary sizes for SyncTeX coordinates mapping
      canvas.dataset.widthPt = page.view[2].toString();
      canvas.dataset.heightPt = page.view[3].toString();
      canvas.dataset.viewportWidth = viewport.width.toString();
      canvas.dataset.viewportHeight = viewport.height.toString();
      
      canvas.width = viewport.width;
      canvas.height = viewport.height;
      const ctx = canvas.getContext('2d');
      if (ctx) {
        // @ts-ignore
        await page.render({ canvasContext: ctx, viewport }).promise;
        canvas.style.opacity = '1';
        canvas.dataset.rendered = 'true';
      }
    } catch (e) {
      console.error('Page render error', e);
    }
  };

  const canvasToPdfPoint = (e: MouseEvent, canvas: HTMLCanvasElement) => {
    const rect = canvas.getBoundingClientRect();
    const x_px = e.clientX - rect.left;
    const y_px = e.clientY - rect.top;

    const wPt = parseFloat(canvas.dataset.widthPt || '595');
    const hPt = parseFloat(canvas.dataset.heightPt || '842');
    return {
      x: (x_px / rect.width) * wPt,
      y: (y_px / rect.height) * hPt
    };
  };

  const handlePdfDblClick = async (e: MouseEvent, pageNum: number, canvas: HTMLCanvasElement) => {
    const point = canvasToPdfPoint(e, canvas);

    try {
      const data = await easytexClient.synctexEdit(projectName(), pageNum, point.x, point.y);
      if (data.line) {
        // If SyncTeX points to a different file in the project, load it!
        if (data.file && data.file !== activeFile()) {
          setActiveFile(data.file);
          setTimeout(() => {
            jumpToLine(data.line, data.column || 1);
          }, 150);
        } else {
          jumpToLine(data.line, data.column || 1);
        }
        setLogs(prev => [...prev, { msg: `🎯 Synced to source: ${data.file}:${data.line}`, lvl: 'info', id: Date.now() }]);
      }
    } catch (err) {
      console.error("Inverse search failed", err);
      const message = err instanceof Error ? err.message : 'SyncTeX inverse search failed';
      setLogs(prev => [...prev, { msg: message, lvl: 'warn', id: Date.now() }]);
    }
  };

  const syncForwardFromCursor = () => {
    if (!editorView) return;
    const pos = editorView.state.selection.main.head;
    const line = editorView.state.doc.lineAt(pos);
    syncForward(line.number, pos - line.from + 1);
  };

  const syncForward = async (line: number, col: number) => {
    try {
      const data = await easytexClient.synctexView(projectName(), line, col, activeFile());
      if (data.page) {
        scrollToPage(data.page);
        highlightPdfCoord(data.page, data.x, data.y);
      }
    } catch (e) {
      console.error("Forward search failed", e);
      const message = e instanceof Error ? e.message : 'SyncTeX forward search failed';
      setLogs(prev => [...prev, { msg: message, lvl: 'warn', id: Date.now() }]);
    }
  };

  const highlightPdfCoord = (page: number, x: number, y: number) => {
    if (!viewerContainer) return;
    const wrap = viewerContainer.querySelector(`[data-n="${page}"]`) as HTMLElement;
    if (!wrap) return;

    let highlight = wrap.querySelector('.synctex-highlight') as HTMLElement;
    if (!highlight) {
      highlight = document.createElement('div');
      highlight.className = 'synctex-highlight';
      wrap.appendChild(highlight);
    }

    const canvas = wrap.querySelector('canvas');
    const wPt = canvas ? parseFloat(canvas.dataset.widthPt || '595') : 595;
    const hPt = canvas ? parseFloat(canvas.dataset.heightPt || '842') : 842;

    const rect = wrap.getBoundingClientRect();
    const px_x = (x / wPt) * rect.width;
    const px_y = (y / hPt) * rect.height;

    highlight.style.left = `${px_x}px`;
    highlight.style.top = `${px_y}px`;
    highlight.style.display = 'block';

    highlight.animate([
      { transform: 'translate(-50%, -50%) scale(2.5)', opacity: 1, border: '4px solid var(--blue)', background: 'rgba(56, 189, 248, 0.2)' },
      { transform: 'translate(-50%, -50%) scale(1.0)', opacity: 0, border: '2px solid var(--blue)', background: 'transparent' }
    ], {
      duration: 1200,
      easing: 'cubic-bezier(0.25, 1, 0.5, 1)',
      fill: 'forwards'
    });
  };

  const runAction = async () => {
    if (isReadOnly()) return;
    const p = projectName();
    if (isBuilding()) {
      await easytexClient.cancel(p);
    } else {
      await easytexClient.run(p);
    }
  };

  const saveAuthToken = () => {
    const token = authTokenInput().trim();
    if (token) {
      window.localStorage.setItem('easytex_admin_token', token);
    } else {
      window.localStorage.removeItem('easytex_admin_token');
    }
    setShowAuthModal(false);
    fetchCapabilities();
    if (projectName()) {
      fetchProjectFiles(projectName());
    } else {
      fetchProjects();
    }
  };



  const updateZoom = (delta: number) => {
    setZoom(prev => Math.max(0.4, Math.min(4.0, delta === 0 ? 1.0 : prev + delta)));
    renderAllPages();
  };

  const scrollHandler = () => {
    if (!viewerContainer) return;
    const wraps = viewerContainer.getElementsByClassName('pg-wrap');
    const threshold = viewerContainer.scrollTop + viewerContainer.offsetHeight * 0.3;
    for (let i = 0; i < wraps.length; i++) {
      const w = wraps[i] as HTMLElement;
      if (w.offsetTop + w.offsetHeight > threshold) {
        setCurPage(i + 1);
        break;
      }
    }
  };

  const scrollToPage = (n: number) => {
    if (!viewerContainer) return;
    const wraps = viewerContainer.getElementsByClassName('pg-wrap');
    const target = wraps[n - 1] as HTMLElement;
    if (target) viewerContainer.scrollTo({ top: target.offsetTop - 20, behavior: 'smooth' });
  };

  return (
    <div class="app-root">
      <header>
        <div style="display:flex;align-items:center;gap:20px">
          <div 
            onClick={() => navigateTo('')} 
            style="cursor:pointer;display:flex;align-items:center;gap:8px"
          >
            <span style="font-weight:800;font-size:18px;letter-spacing:-0.02em;color:var(--blue)">
              EasyTex<span style="color:var(--txt)">.</span>
            </span>
          </div>
          <div style="width:1px;height:24px;background:var(--brd)"></div>
          <Show when={projectName()}>
            <div style="display:flex;align-items:center;gap:12px">
              <span style="font-weight:600;font-size:14px;color:var(--sub)">{projectName()}</span>
              <div id="dot" classList={{ online: isConnected(), building: isBuilding() }}></div>
            </div>
          </Show>
        </div>

        <div style="display:flex;align-items:center;gap:16px">
          <Show when={!projectName() && !isReadOnly()}>
             <button class="btn btn-blue" onClick={createProject}>
               <Plus size={16} /> New Project
             </button>
          </Show>
        </div>
      </header>

      <main style={{
        "--left-width": `${leftWidth()}%`,
        "--top-height": `${topHeight()}%`,
        "--sidebar-width": `${sidebarWidth()}px`
      }}>
        <Show when={!projectName()} fallback={
          <>
            <div 
              id="left-pane" 
              classList={{ 'editor-hidden': !showEditor() }}
              style={{ width: `${leftWidth()}%` }}
            >
              {/* Local Header/Toolbar for Explorer & Editor */}
              <div style="display:flex;align-items:center;justify-content:space-between;padding: 8px 16px;background:#101216;border-bottom:1px solid var(--brd);flex-shrink:0;user-select:none;flex-wrap:wrap;gap:12px;">
                <div style="display:flex;align-items:center;gap:12px">
                  <div style="display:flex;align-items:center;gap:8px">
                    <span style="font-size:14px">💻</span>
                    <span style="font-size:11px;font-weight:700;text-transform:uppercase;letter-spacing:0.1em;color:var(--txt)">Working Zone</span>
                  </div>
                  
                  {/* Build stats */}
                  <Show when={stats()}>
                    <div id="stats" style="display:flex;gap:6px;margin-left:8px">
                      <div class="stat-badge" title="Build Time" style="padding: 2px 6px; font-size: 10px; height: 20px; line-height: 16px; display: flex; align-items: center; gap: 4px;">
                        ⚡ <b>{stats()?.time}</b>
                      </div>
                      <div class="stat-badge" title="Word Count" style="padding: 2px 6px; font-size: 10px; height: 20px; line-height: 16px; display: flex; align-items: center; gap: 4px;">
                        📝 <b>{stats()?.words}</b> words
                      </div>
                      <div class="stat-badge" title="PDF Size" style="padding: 2px 6px; font-size: 10px; height: 20px; line-height: 16px; display: flex; align-items: center; gap: 4px;">
                        📦 <b>{stats()?.size}</b>
                      </div>
                    </div>
                  </Show>
                </div>

                {/* Actions & Toggle */}
                <div style="display:flex;align-items:center;gap:6px">
                  <Show when={!isReadOnly()}>
                    <button 
                      id="btn-run"
                      class="btn" 
                      classList={{ 'btn-red': isBuilding(), 'btn-blue': !isBuilding() }}
                      onClick={runAction}
                      style="padding: 4px 10px; font-size: 11px; height: 26px; display: flex; align-items: center; gap: 4px;"
                    >
                      {isBuilding() ? <Square size={12} fill="currentColor" /> : <Play size={12} fill="currentColor" />}
                      {isBuilding() ? 'Stop' : 'Run'}
                    </button>
                  </Show>
                  
                  <button 
                    class="btn" 
                    onClick={() => setShowErrorModal(true)}
                    style="padding: 4px 10px; font-size: 11px; height: 26px; display: flex; align-items: center; gap: 4px; border-color: rgba(251, 113, 133, 0.2); background: rgba(251, 113, 133, 0.03); color: var(--red);"
                  >
                    <span>⚠️</span> Errors ({errorCount()})
                  </button>
                  
                  <Show when={capabilities().format && !isReadOnly()}>
                    <button 
                      class="btn" 
                      onClick={async () => {
                        try {
                          await easytexClient.format(projectName());
                        } catch (e) {
                          console.error("Format failed:", e);
                        }
                      }}
                      style="padding: 4px 10px; font-size: 11px; height: 26px; display: flex; align-items: center; gap: 4px;"
                    >
                      <span>✨</span> Format
                    </button>
                  </Show>

                  <Show when={capabilities().lint}>
                    <button 
                      class="btn" 
                      onClick={runLint}
                      style="padding: 4px 10px; font-size: 11px; height: 26px; display: flex; align-items: center; gap: 4px;"
                    >
                      Lint
                    </button>
                  </Show>
                  
                  <button 
                    class="btn" 
                    onClick={fetchConfig}
                    style="padding: 4px 10px; font-size: 11px; height: 26px; display: flex; align-items: center; gap: 4px;"
                  >
                    <span>⚙️</span> Settings
                  </button>
                  
                  <Show when={!isReadOnly()}>
                    <button 
                      class="btn" 
                      onClick={async () => {
                        try {
                          await easytexClient.clean(projectName());
                        } catch (e) {
                          console.error("Clean failed:", e);
                        }
                      }}
                      style="padding: 4px 10px; font-size: 11px; height: 26px; display: flex; align-items: center; gap: 4px;"
                    >
                      <span>🧹</span> Clean
                    </button>
                  </Show>
                  
                  <div style="position:relative;">
                    <button 
                      class="btn" 
                      onClick={() => setShowExportMenu(!showExportMenu())}
                      style="padding: 4px 10px; font-size: 11px; height: 26px; display: flex; align-items: center; gap: 4px;"
                    >
                      <Download size={13} /> Export
                    </button>
                    <Show when={showExportMenu()}>
                      <div style="position:absolute; right:0; top:32px; min-width:180px; background:var(--panel); border:1px solid var(--brd); border-radius:8px; box-shadow:0 14px 36px rgba(0,0,0,0.45); padding:6px; z-index:500; display:flex; flex-direction:column; gap:4px;">
                        <button class="btn" onClick={downloadLatestPdf} style="justify-content:flex-start; height:30px; padding:6px 8px; font-size:12px; border:none; background:transparent;">
                          Download
                        </button>
                        <button class="btn" onClick={openDownloadsFromExport} style="justify-content:flex-start; height:30px; padding:6px 8px; font-size:12px; border:none; background:transparent;">
                          View build history
                        </button>
                      </div>
                    </Show>
                  </div>

                  <div style="width: 1px; height: 16px; background: var(--brd); margin: 0 4px;"></div>

                  <button 
                    class="btn" 
                    onClick={() => setShowEditor(!showEditor())} 
                    style="padding: 4px 10px; font-size: 11px; height: 26px; display: flex; align-items: center; gap: 6px; cursor: pointer; background: rgba(255,255,255,0.02); border: 1px solid var(--brd); border-radius: 4px;"
                  >
                    <span>{showEditor() ? "Hide editor" : "Show editor"}</span>
                  </button>
                </div>
              </div>

              <div 
                id="left-pane-top"
                style={{ height: `${topHeight()}px` }}
              >
                <div 
                  id="sidebar"
                  style={{ width: `${sidebarWidth()}px` }}
                >
                  <div class="sidebar-header">
                    <span class="sidebar-title">Files</span>
                    <Show when={!isReadOnly()}>
                      <button class="sidebar-btn" onClick={createFile} title="New file">
                        <Plus size={14} />
                      </button>
                    </Show>
                  </div>
                  <div class="file-list" style={{ "padding": "12px 8px", "display": "flex", "flex-direction": "column", "gap": "2px", "overflow-y": "auto", "height": "calc(100% - 50px)" }}>
                    <For each={buildFileTree(files())}>
                      {(node) => <FileTreeNode node={node} depth={0} />}
                    </For>
                  </div>
                </div>

                {/* Sidebar Splitter */}
                <div class="resizer-sidebar" onMouseDown={initSidebarResize}></div>

                <div id="editor-container" ref={editorContainer}></div>
              </div>
              
              {/* Horizontal Splitter between Top Row and Console */}
              <Show when={showEditor()}>
                <div class="resizer-y" onMouseDown={initYResize}></div>
              </Show>

              <div id="l">
                <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:12px;padding-bottom:12px;border-bottom:1px solid var(--brd);padding: 16px 16px 12px 16px;flex-shrink:0;">
                  <span style="font-size:11px;font-weight:700;text-transform:uppercase;letter-spacing:0.1em;color:var(--sub)">Console de Build</span>
                  <div style="display:flex;gap:4px">
                    <div style="width:8px;height:8px;border-radius:50%;background:#ef4444"></div>
                    <div style="width:8px;height:8px;border-radius:50%;background:#f59e0b"></div>
                    <div style="width:8px;height:8px;border-radius:50%;background:#10b981"></div>
                  </div>
                </div>
                <div style="flex:1;overflow-y:scroll;padding: 0 16px 16px 16px;font-family:'JetBrains Mono',monospace;font-size:12px;gap:2px;" ref={logContainer} onScroll={handleLogScroll}>
                  <For each={logs()}>
                    {(log) => (
                      <div class={`log-line log-${log.lvl}`}>
                        {log.msg}
                      </div>
                    )}
                  </For>
                </div>
              </div>
            </div>

            {/* Vertical Splitter between Left Pane and PDF Viewer */}
            <div class="resizer-x" onMouseDown={initXResize}></div>

            <div id="v" ref={viewerContainer} onScroll={scrollHandler}>
              {/* PDF Pages will be injected here */}
              <Show when={!totalPage()}>
                 <div style="display:flex;flex-direction:column;align-items:center;justify-content:center;height:100%;color:var(--sub);gap:16px;animation:fadeIn 0.5s ease-out">
                    <div style="width:40px;height:40px;border-radius:50%;background:var(--blue);opacity:0.6;animation:pulse 2s infinite"></div>
                    <span style="font-size:14px;letter-spacing:0.02em">Generating initial PDF preview...</span>
                    <Show when={!isReadOnly()}>
                      <button class="btn btn-blue" style="margin-top:8px" onClick={runAction}>Run build</button>
                    </Show>
                 </div>
              </Show>
            </div>

            <div id="pg-nav">
              <Show when={previewAge()}>
                <div style="display:flex;align-items:center;gap:8px;border-right:1px solid var(--brd);padding-right:12px;max-width:220px;">
                  <span style="width:8px;height:8px;border-radius:50%;background:var(--green);flex-shrink:0"></span>
                  <span style="font-size:11px;font-weight:700;color:var(--txt);white-space:nowrap">{previewAge()}</span>
                </div>
              </Show>
              <div style="display:flex;align-items:center;gap:12px;border-right:1px solid var(--brd);padding-right:12px;">
                <div class="pg-btn" onClick={() => updateZoom(-0.1)}>-</div>
                <span id="zoom-val" style="min-width:40px;text-align:center;font-size:12px;font-weight:700">{Math.round(zoom() * 100)}%</span>
                <div class="pg-btn" onClick={() => updateZoom(0.1)}>+</div>
              </div>
              <div style="display:flex;align-items:center;gap:8px">
                <div class="pg-btn" onClick={() => scrollToPage(curPage() - 1)}><ChevronLeft size={18} /></div>
                <span 
                  style="font-size:12px;font-weight:700;color:var(--txt);cursor:pointer"
                  onClick={() => {
                    const p = prompt("Jump to page:", curPage().toString());
                    if (p) {
                      const n = parseInt(p);
                      if (!isNaN(n)) scrollToPage(n);
                    }
                  }}
                >
                  PAGE <b style="color:var(--txt)">{curPage()}</b> <span style="color:var(--sub);font-weight:400">/</span> {totalPage()}
                </span>
                <div class="pg-btn" onClick={() => scrollToPage(curPage() + 1)}><ChevronRight size={18} /></div>
              </div>
            </div>
          </>
        }>
          <div class="card-grid">
            <For each={projects()}>
              {(p) => (
                <div class="project-card" onClick={() => navigateTo(p)} style="cursor:pointer; position: relative;">
                  <div class="project-icon">📄</div>
                  <div class="card-content" style="flex: 1;">
                    <h3>{p}</h3>
                    <div class="path">Project</div>
                  </div>
                  {p !== 'demo' && !isReadOnly() && (
                    <button 
                      class="delete-btn" 
                      onClick={(e) => deleteProject(p, e)} 
                      title="Delete project"
                    >
                      🗑️
                    </button>
                  )}
                </div>
              )}
            </For>
            <Show when={projects().length === 0}>
               <div style="grid-column: 1 / -1; text-align:center; padding:100px; color:var(--sub)">
                 <Layout size={48} style="margin: 0 auto 20px; opacity:0.2" />
                 <p>No projects found. Create your first one!</p>
               </div>
            </Show>
          </div>
        </Show>
      </main>

      <Show when={showAuthModal()}>
        <div id="modal-overlay" onClick={() => setShowAuthModal(false)}>
          <div class="modal-box" onClick={(e) => e.stopPropagation()} style="max-height: 75vh; display: flex; flex-direction: column; width: 480px;">
            <div style="display:flex; justify-content:space-between; align-items:center; border-bottom:1px solid var(--brd); padding-bottom:16px;">
              <h3 style="font-size:16px; font-weight:700; color:var(--txt)">Authentication Required</h3>
              <button class="sidebar-btn" onClick={() => setShowAuthModal(false)} style="font-size: 20px;">×</button>
            </div>
            <div style="margin:20px 0; display:flex; flex-direction:column; gap:12px;">
              <label style="font-size:12px; color:var(--sub);">EasyTex admin token</label>
              <input
                type="password"
                value={authTokenInput()}
                onInput={(e) => setAuthTokenInput(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') saveAuthToken();
                }}
                style="height:36px; background:#0b0d11; border:1px solid var(--brd); border-radius:8px; padding:0 12px; color:var(--txt); outline:none;"
                autofocus
              />
              <p style="font-size:12px; color:var(--sub); line-height:1.5; margin:0;">
                The token is stored locally in this browser and sent as a bearer token to protected routes.
              </p>
            </div>
            <div style="display:flex; justify-content:flex-end; gap:8px; border-top:1px solid var(--brd); padding-top:16px;">
              <button class="btn" onClick={() => setShowAuthModal(false)}>Cancel</button>
              <button class="btn btn-blue" onClick={saveAuthToken}>Save Token</button>
            </div>
          </div>
        </div>
      </Show>

      <Show when={showErrorModal()}>
        <div id="modal-overlay" onClick={() => setShowErrorModal(false)}>
          <div class="modal-box" onClick={(e) => e.stopPropagation()} style="max-height: 80vh; display: flex; flex-direction: column; width: 680px;">
            <div style="display:flex; justify-content:space-between; align-items:center; border-bottom:1px solid var(--brd); padding-bottom:16px;">
              <h3 style="font-size:16px; font-weight:700; color:var(--txt)">Compilation Report</h3>
              <button class="sidebar-btn" onClick={() => setShowErrorModal(false)} style="font-size: 20px;">×</button>
            </div>
            <div style="flex:1; overflow-y:auto; margin: 20px 0; padding-right: 8px; display:flex; flex-direction:column; gap:12px;">
              <Show when={errorCount() === 0 && warningDiagnostics().length === 0 && warningLogs().length === 0}>
                <div style="text-align:center; padding:40px; color:var(--sub)">
                  <span style="font-size:32px">🎉</span>
                  <p style="margin-top:12px; font-weight:500">No errors or warnings detected.</p>
                </div>
              </Show>
              
              <Show when={errorDiagnostics().length > 0}>
                <h4 style="color:var(--red); font-size:12px; text-transform:uppercase; letter-spacing:0.05em; font-weight:700; margin-bottom: 4px;">Errors ({errorDiagnostics().length})</h4>
                <div style="display:flex; flex-direction:column; gap:8px;">
                  <For each={errorDiagnostics()}>
                    {(diagnostic) => (
                      <div onClick={() => openDiagnostic(diagnostic)} style="background:rgba(251, 113, 133, 0.03); border:1px solid rgba(251, 113, 133, 0.1); padding:12px; border-radius:8px; font-family:'JetBrains Mono',monospace; font-size:11px; color:var(--red); line-height:1.5; white-space:pre-wrap; word-break:break-all; cursor:pointer;">
                        <Show when={diagnostic.file || diagnostic.line}>
                          <div style="font-family:'Inter',sans-serif; font-size:11px; font-weight:700; margin-bottom:6px; color:var(--txt)">
                            {diagnostic.file || activeFile()}{diagnostic.line ? `:${diagnostic.line}` : ''}{diagnostic.column ? `:${diagnostic.column}` : ''}
                          </div>
                        </Show>
                        {diagnostic.message}
                      </div>
                    )}
                  </For>
                </div>
              </Show>

              <Show when={errorDiagnostics().length === 0 && errorLogs().length > 0}>
                <h4 style="color:var(--red); font-size:12px; text-transform:uppercase; letter-spacing:0.05em; font-weight:700; margin-bottom: 4px;">Errors ({errorLogs().length})</h4>
                <div style="display:flex; flex-direction:column; gap:8px;">
                  <For each={errorLogs()}>
                    {(log) => (
                      <div style="background:rgba(251, 113, 133, 0.03); border:1px solid rgba(251, 113, 133, 0.1); padding:12px; border-radius:8px; font-family:'JetBrains Mono',monospace; font-size:11px; color:var(--red); line-height:1.5; white-space:pre-wrap; word-break:break-all;">
                        {log.msg}
                      </div>
                    )}
                  </For>
                </div>
              </Show>

              <Show when={warningDiagnostics().length > 0}>
                <h4 style="color:var(--gold); font-size:12px; text-transform:uppercase; letter-spacing:0.05em; font-weight:700; margin-bottom: 4px; margin-top: 12px;">Warnings ({warningDiagnostics().length})</h4>
                <div style="display:flex; flex-direction:column; gap:8px;">
                  <For each={warningDiagnostics()}>
                    {(diagnostic) => (
                      <div onClick={() => openDiagnostic(diagnostic)} style="background:rgba(251, 191, 36, 0.03); border:1px solid rgba(251, 191, 36, 0.1); padding:12px; border-radius:8px; font-family:'JetBrains Mono',monospace; font-size:11px; color:var(--gold); line-height:1.5; white-space:pre-wrap; word-break:break-all; cursor:pointer;">
                        <Show when={diagnostic.file || diagnostic.line}>
                          <div style="font-family:'Inter',sans-serif; font-size:11px; font-weight:700; margin-bottom:6px; color:var(--txt)">
                            {diagnostic.file || activeFile()}{diagnostic.line ? `:${diagnostic.line}` : ''}{diagnostic.column ? `:${diagnostic.column}` : ''}
                          </div>
                        </Show>
                        {diagnostic.message}
                      </div>
                    )}
                  </For>
                </div>
              </Show>

              <Show when={warningDiagnostics().length === 0 && warningLogs().length > 0}>
                <h4 style="color:var(--gold); font-size:12px; text-transform:uppercase; letter-spacing:0.05em; font-weight:700; margin-bottom: 4px; margin-top: 12px;">Warnings ({warningLogs().length})</h4>
                <div style="display:flex; flex-direction:column; gap:8px;">
                  <For each={warningLogs()}>
                    {(log) => (
                      <div style="background:rgba(251, 191, 36, 0.03); border:1px solid rgba(251, 191, 36, 0.1); padding:12px; border-radius:8px; font-family:'JetBrains Mono',monospace; font-size:11px; color:var(--gold); line-height:1.5; white-space:pre-wrap; word-break:break-all;">
                        {log.msg}
                      </div>
                    )}
                  </For>
                </div>
              </Show>
            </div>
            <div style="display:flex; justify-content:flex-end; border-top:1px solid var(--brd); padding-top:16px;">
              <button class="btn" onClick={() => setShowErrorModal(false)}>Close</button>
            </div>
          </div>
        </div>
      </Show>

      <Show when={showLintModal()}>
        <div id="modal-overlay" onClick={() => setShowLintModal(false)}>
          <div class="modal-box" onClick={(e) => e.stopPropagation()} style="max-height: 80vh; display: flex; flex-direction: column; width: 760px;">
            <div style="display:flex; justify-content:space-between; align-items:center; border-bottom:1px solid var(--brd); padding-bottom:16px;">
              <h3 style="font-size:16px; font-weight:700; color:var(--txt)">ChkTeX lint</h3>
              <button class="sidebar-btn" onClick={() => setShowLintModal(false)} style="font-size: 20px;">×</button>
            </div>
            <div style="flex:1; overflow-y:auto; margin: 20px 0; display:flex; flex-direction:column; gap:12px;">
              <Show when={isLinting()}>
                <div style="text-align:center; padding:36px; color:var(--sub); font-size:13px;">Analysis in progress...</div>
              </Show>
              <Show when={!isLinting() && lintResult()}>
                <div style={{
                  "display": "inline-flex",
                  "align-items": "center",
                  "align-self": "flex-start",
                  "gap": "8px",
                  "font-size": "12px",
                  "font-weight": "700",
                  "color": lintResult()?.ok ? "var(--green)" : "var(--gold)"
                }}>
                  {lintResult()?.ok ? 'No issues reported' : `ChkTeX finished with code ${lintResult()?.status ?? 'unknown'}`}
                </div>
                <pre style="background:#0b0d11; border:1px solid var(--brd); border-radius:8px; padding:14px; color:var(--txt); font-family:'JetBrains Mono',monospace; font-size:12px; line-height:1.55; white-space:pre-wrap; word-break:break-word; min-height:180px;">{`${lintResult()?.stdout || ''}${lintResult()?.stderr ? `\n${lintResult()?.stderr}` : ''}`.trim() || 'No output.'}</pre>
              </Show>
            </div>
            <div style="display:flex; justify-content:flex-end; border-top:1px solid var(--brd); padding-top:16px;">
              <button class="btn" onClick={() => setShowLintModal(false)}>Close</button>
            </div>
          </div>
        </div>
      </Show>

      <Show when={showDownloadsModal()}>
        <div id="modal-overlay" onClick={() => setShowDownloadsModal(false)}>
          <div class="modal-box" onClick={(e) => e.stopPropagation()} style="max-height: 75vh; display: flex; flex-direction: column; width: 560px;">
            <div style="display:flex; justify-content:space-between; align-items:center; border-bottom:1px solid var(--brd); padding-bottom:16px;">
              <h3 style="font-size:16px; font-weight:700; color:var(--txt)">Successful Builds</h3>
              <button class="sidebar-btn" onClick={() => setShowDownloadsModal(false)} style="font-size: 20px;">×</button>
            </div>
            <div style="flex:1; overflow-y:auto; margin: 16px 0; display:flex; flex-direction:column; gap:8px;">
              <Show when={successBuilds().length === 0}>
                <div style="text-align:center; padding:36px; color:var(--sub); font-size:13px;">
                  No successful builds available.
                </div>
              </Show>
              <For each={successBuilds()}>
                {(build, index) => (
                  <div style="display:flex; align-items:center; justify-content:space-between; gap:12px; padding:12px; border:1px solid var(--brd); background:rgba(255,255,255,0.02); border-radius:8px;">
                    <div style="min-width:0; display:flex; flex-direction:column; gap:4px;">
                      <div style="display:flex; align-items:center; gap:8px;">
                        <span style="font-size:12px; font-weight:700; color:var(--txt);">{index() === 0 ? 'Latest success' : `Success #${index() + 1}`}</span>
                        <span style="font-size:11px; color:var(--sub);">{formatBuildAge(build.built_at_ms)}</span>
                      </div>
                      <div style="font-family:'JetBrains Mono',monospace; font-size:10px; color:var(--sub); overflow:hidden; text-overflow:ellipsis; white-space:nowrap;">
                        {build.run} · {formatBytes(build.pdf_size_bytes)}
                      </div>
                    </div>
                    <button class="btn btn-blue" onClick={() => downloadBuild(build.run)} style="height:28px; padding:4px 9px; font-size:11px; flex-shrink:0;">
                      <Download size={13} /> DL
                    </button>
                  </div>
                )}
              </For>
            </div>
            <div style="display:flex; justify-content:flex-end; border-top:1px solid var(--brd); padding-top:16px;">
              <button class="btn" onClick={() => setShowDownloadsModal(false)}>Close</button>
            </div>
          </div>
        </div>
      </Show>
      <Show when={showConfigModal()}>
        <div id="modal-overlay" onClick={() => setShowConfigModal(false)}>
          <div id="modal" class="modal-box" onClick={(e) => e.stopPropagation()} style="max-height: 85vh; display: flex; flex-direction: column; width: 680px;">
            <div style="display:flex; justify-content:space-between; align-items:center; border-bottom:1px solid var(--brd); padding-bottom:16px;">
              <h3 style="font-size:16px; font-weight:700; color:var(--txt)">Settings</h3>
              <button class="sidebar-btn" onClick={() => setShowConfigModal(false)} style="font-size: 20px;">×</button>
            </div>
            <div style="flex:1; overflow-y:auto; margin: 20px 0; display:flex; flex-direction:column; gap:12px;">
              <label style="font-size:12px; color:var(--sub);">Edit the project configuration file <code>EasyTex.toml</code>:</label>
              <textarea 
                id="cfg-txt"
                value={configRaw()} 
                onInput={(e) => setConfigRaw(e.currentTarget.value)}
                style="flex:1; background:#0b0d11; border:1px solid var(--brd); border-radius:8px; padding:14px; color:var(--txt); font-family:'JetBrains Mono',monospace; font-size:12px; line-height:1.55; min-height:300px; resize:vertical; outline:none;"
                disabled={isReadOnly()}
                placeholder="# EasyTex project settings..."
              />
              <Show when={configError()}>
                <div id="cfg-err" style="color:var(--red); font-size:12px; font-weight:500; margin-top:4px;">
                  {configError()}
                </div>
              </Show>
            </div>
            <div style="display:flex; justify-content:space-between; align-items:center; border-top:1px solid var(--brd); padding-top:16px;">
              <span style="font-size:11px; color:var(--sub);">
                {isReadOnly() ? "Read-only mode enabled" : ""}
              </span>
              <div style="display:flex; gap:8px;">
                <button class="btn" onClick={() => setShowConfigModal(false)}>Cancel</button>
                <Show when={!isReadOnly()}>
                  <button 
                    class="btn btn-blue" 
                    onClick={saveConfig} 
                    disabled={isSavingConfig()}
                  >
                    {isSavingConfig() ? "Saving..." : "Save Changes"}
                  </button>
                </Show>
              </div>
            </div>
          </div>
        </div>
      </Show>
    </div>
  );
}

export default App;
