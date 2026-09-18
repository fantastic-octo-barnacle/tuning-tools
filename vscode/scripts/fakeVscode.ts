// Just enough of the `vscode` API to run the extension headlessly in scripts/harness.ts.

import { join } from "node:path";

type Listener<T> = (value: T) => unknown;

export class Disposable {
  constructor(private readonly fn: () => void) {}
  dispose() {
    this.fn();
  }
}

export class Uri {
  constructor(readonly fsPath: string) {}
  static file(path: string) {
    return new Uri(path);
  }
  static joinPath(base: Uri, ...parts: string[]) {
    return new Uri(join(base.fsPath, ...parts));
  }
  toString() {
    return `https://file+.vscode-resource.test${this.fsPath}`;
  }
}

export enum ViewColumn {
  Active = -1,
}

function emitter<T>() {
  const listeners = new Set<Listener<T>>();
  const event = (l: Listener<T>) => {
    listeners.add(l);
    return new Disposable(() => listeners.delete(l));
  };
  return { event, fire: (v: T) => listeners.forEach((l) => l(v)) };
}

/** What the harness inspects and drives */
export const harness = {
  commands: new Map<string, () => unknown>(),
  configProviders: [] as { resolveDebugConfigurationWithSubstitutedVariables?: (f: unknown, c: any) => Promise<any> }[],
  started: emitter<any>(),
  terminated: emitter<any>(),
  panels: [] as any[],
  settings: {} as Record<string, unknown>,
  info: [] as string[],
  errors: [] as string[],
  log: [] as string[],
  launch: [] as unknown[],
};

export const commands = {
  registerCommand(name: string, fn: () => unknown) {
    harness.commands.set(name, fn);
    return new Disposable(() => harness.commands.delete(name));
  },
};

export const window = {
  createOutputChannel: () => ({ appendLine: (l: string) => harness.log.push(l), dispose() {} }),
  showErrorMessage: async (m: string) => void harness.errors.push(m),
  showInformationMessage: async (m: string) => void harness.info.push(m),
  showOpenDialog: async () => undefined,
  registerWebviewPanelSerializer: () => new Disposable(() => {}),
  createWebviewPanel(_type: string, _title: string, _column: unknown, options: unknown) {
    const fromPage = emitter<any>();
    const disposed = emitter<void>();
    const panel = {
      createOptions: options,
      toPage: [] as any[],
      send: (m: any) => fromPage.fire(m),
      webview: {
        options: {},
        html: "",
        cspSource: "https://file+.vscode-resource.test",
        asWebviewUri: (u: Uri) => u.toString(),
        postMessage: async (m: any) => {
          panel.toPage.push(m);
          return true;
        },
        onDidReceiveMessage: fromPage.event,
      },
      reveal() {},
      onDidDispose: disposed.event,
      dispose: () => disposed.fire(),
    };
    harness.panels.push(panel);
    return panel;
  },
};

export const workspace = {
  workspaceFolders: undefined as unknown,
  getConfiguration(section: string) {
    return {
      get: (key: string) => (section === "launch" && key === "configurations" ? harness.launch : harness.settings[`${section}.${key}`]),
    };
  },
};

export const debug = {
  activeDebugSession: undefined,
  registerDebugConfigurationProvider(_type: string, provider: any) {
    harness.configProviders.push(provider);
    return new Disposable(() => {});
  },
  registerDebugAdapterTrackerFactory: () => new Disposable(() => {}),
  onDidStartDebugSession: harness.started.event,
  onDidTerminateDebugSession: harness.terminated.event,
};
