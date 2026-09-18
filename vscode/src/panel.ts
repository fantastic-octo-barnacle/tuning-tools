// The studio in a webview panel. The page is the same React app as the desktop build (vscode/media,
// from `npm run build:webview`); its `vscode` host posts calls here. This side answers the ones only
// VS Code can (dialogs, launch configurations, the probe's owner) and relays the rest to a
// studio-server child process, whose sample frames go on to the page as ArrayBuffers.

import { randomBytes } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import * as vscode from "vscode";
import { launchDefaults } from "./launch";
import { ProbeGuard, ProbeHolder } from "./probeGuard";
import { findServer } from "./serverPath";
import { StudioServer } from "./server";

export const VIEW_TYPE = "tuningStudio";
const STORAGE_KEY = "tuningStudio.storage";
const LAST_ELF_KEY = "tuningStudio.lastElf";
/** Where recordings go, under the first workspace folder */
const RECORDINGS_DIR = [".tuning-studio", "recordings"];

const DEBUGGER_UNSUPPORTED =
  "Sharing the probe with a debug session is not supported: each read through the debugger takes " +
  "about 14 ms and probe-rs halts the core around it. Stop the debug session to use the probe here, or connect over USB.";

interface Notice {
  id: number;
  message: string;
  reconnect: boolean;
}

/** Messages from the page */
type FromPage =
  | { type: "call"; id: number; method: string; params: Record<string, unknown> }
  | { type: "storage"; key: string; value: string };

export class StudioPanel implements ProbeHolder {
  static current: StudioPanel | undefined;

  private server: StudioServer | null = null;
  /** The latest session the page started, and whether it may hold the probe */
  private session: { id: number; carrier: string; live: boolean } | null = null;
  /** The debug session the probe was handed to, until it ends */
  private releasedFor: string | null = null;
  private notice: Notice | null = null;
  private nextNotice = 1;
  private readonly disposables: vscode.Disposable[] = [];

  static show(context: vscode.ExtensionContext, guard: ProbeGuard, log: vscode.OutputChannel) {
    if (StudioPanel.current) {
      StudioPanel.current.panel.reveal();
      return;
    }
    const panel = vscode.window.createWebviewPanel(VIEW_TYPE, "Tuning Studio", vscode.ViewColumn.Active, {
      // The scope keeps its sample history in the page
      retainContextWhenHidden: true,
    });
    StudioPanel.current = new StudioPanel(panel, context, guard, log);
  }

  /** A panel VS Code kept across a reload */
  static revive(panel: vscode.WebviewPanel, context: vscode.ExtensionContext, guard: ProbeGuard, log: vscode.OutputChannel) {
    StudioPanel.current?.panel.dispose();
    StudioPanel.current = new StudioPanel(panel, context, guard, log);
  }

  private constructor(
    private readonly panel: vscode.WebviewPanel,
    private readonly context: vscode.ExtensionContext,
    private readonly guard: ProbeGuard,
    private readonly log: vscode.OutputChannel,
  ) {
    const media = vscode.Uri.joinPath(context.extensionUri, "media");
    panel.webview.options = { enableScripts: true, localResourceRoots: [media] };
    panel.webview.html = this.html(media);
    this.disposables.push(
      guard.add(this),
      panel.webview.onDidReceiveMessage((m: FromPage) => this.receive(m)),
      panel.onDidDispose(() => this.dispose()),
    );
    try {
      this.startServer();
    } catch (e) {
      // Calls try again, and the page shows why they fail
      void vscode.window.showErrorMessage(`Tuning Studio: ${e instanceof Error ? e.message : e}`);
    }
  }

  // ProbeHolder

  holdsProbe(): boolean {
    return this.session?.carrier === "probe" && this.session.live;
  }

  async releaseProbe(name: string) {
    if (!this.server || !this.holdsProbe()) return;
    await this.server.call("session_disconnect");
    if (this.session) this.session.live = false;
    this.releasedFor = name;
    this.setNotice(`Released the probe to the debug session "${name}".`, false);
    void vscode.window.showInformationMessage(`Tuning Studio released the probe for the debug session "${name}".`);
  }

  debugSessionsChanged() {
    if (this.releasedFor && !this.guard.blockedBy()) {
      this.setNotice(`The debug session "${this.releasedFor}" ended; the probe is free.`, true);
      this.releasedFor = null;
    }
    this.postStatus();
  }

  // The page

  private status() {
    const blocker = this.guard.blockedBy();
    return {
      sources: {
        probe: blocker
          ? {
              available: false,
              reason: `The debug session "${blocker}" is using the probe. Stop it to connect through the probe here, or connect over USB.`,
            }
          : { available: true, reason: null },
        serial: { available: true, reason: null },
        debugger: { available: false, reason: DEBUGGER_UNSUPPORTED },
      },
      notice: this.notice,
      features: {
        record: { available: true, reason: null },
        exportCsv: { available: true, reason: null },
        stream: { available: true, reason: null },
        reveal: { available: true, reason: null },
      },
    };
  }

  private setNotice(message: string, reconnect: boolean) {
    this.notice = { id: this.nextNotice++, message, reconnect };
    this.postStatus();
  }

  private postStatus() {
    void this.panel.webview.postMessage({ type: "status", status: this.status() });
  }

  private async receive(m: FromPage) {
    if (m.type === "storage") {
      const stored = this.context.workspaceState.get<Record<string, string>>(STORAGE_KEY) ?? {};
      stored[m.key] = m.value;
      await this.context.workspaceState.update(STORAGE_KEY, stored);
      return;
    }
    if (m.type !== "call") return;
    try {
      const result = await this.call(m.method, m.params ?? {});
      void this.panel.webview.postMessage({ type: "result", id: m.id, ok: true, result: result ?? null });
    } catch (e) {
      const error = e instanceof Error ? e.message : String(e);
      void this.panel.webview.postMessage({ type: "result", id: m.id, ok: false, error });
    }
  }

  private async call(method: string, params: Record<string, unknown>): Promise<unknown> {
    switch (method) {
      case "host_status":
        return this.status();
      case "startup":
        return this.startup();
      case "pick_elf":
        return this.pickElf();
      case "session_connect":
        return this.connect(params);
      case "recording_start":
        return this.serverCall(method, { ...params, dir: this.recordingsDir() });
      case "pick_recording_path":
        return this.pickSave("Record to", "Record", this.recordingsDir(), { "MCAP recording": ["mcap"] });
      case "pick_csv_path":
        return this.pickSave("Export CSV", "Export", String(params.suggested ?? ""), { CSV: ["csv"] });
      case "pick_recording":
        return this.pickRecording();
      case "reveal":
        await vscode.commands.executeCommand("revealFileInOS", vscode.Uri.file(String(params.path)));
        return null;
      case "open_elf": {
        const result = await this.serverCall(method, params);
        await this.context.workspaceState.update(LAST_ELF_KEY, params.path);
        return result;
      }
      default:
        return this.serverCall(method, params);
    }
  }

  private startup() {
    const launch = launchDefaults();
    const last = this.context.workspaceState.get<string>(LAST_ELF_KEY);
    const elfPath = last && existsSync(last) ? last : (launch?.elfPath ?? null);
    if (launch) this.log.appendLine(`launch configuration "${launch.name}": chip ${launch.chip}, ELF ${launch.elfPath}`);
    return {
      elfPath,
      connect: null,
      connectDefaults: launch
        ? { chip: launch.chip ?? undefined, probe: launch.probe ?? undefined, speedKhz: launch.speedKhz ?? undefined }
        : null,
      watches: [],
    };
  }

  private async pickElf(): Promise<string | null> {
    const near = this.context.workspaceState.get<string>(LAST_ELF_KEY) ?? launchDefaults()?.elfPath;
    const picked = await vscode.window.showOpenDialog({
      title: "Open firmware ELF",
      openLabel: "Open ELF",
      canSelectMany: false,
      canSelectFolders: false,
      defaultUri: near ? vscode.Uri.file(dirname(near)) : vscode.workspace.workspaceFolders?.[0]?.uri,
    });
    return picked?.[0]?.fsPath ?? null;
  }

  /** `<workspace>/.tuning-studio/recordings`; null outside a workspace, so the server picks */
  private recordingsDir(): string | null {
    const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    return folder ? join(folder, ...RECORDINGS_DIR) : null;
  }

  private async pickSave(
    title: string,
    saveLabel: string,
    near: string | null,
    filters: Record<string, string[]>,
  ): Promise<string | null> {
    const picked = await vscode.window.showSaveDialog({
      title,
      saveLabel,
      filters,
      defaultUri: near ? vscode.Uri.file(near) : vscode.workspace.workspaceFolders?.[0]?.uri,
    });
    return picked?.fsPath ?? null;
  }

  private async pickRecording(): Promise<string | null> {
    const dir = this.recordingsDir();
    const picked = await vscode.window.showOpenDialog({
      title: "Export a recording as CSV",
      openLabel: "Export",
      canSelectMany: false,
      canSelectFolders: false,
      filters: { "MCAP recording": ["mcap"] },
      defaultUri: dir && existsSync(dir) ? vscode.Uri.file(dir) : vscode.workspace.workspaceFolders?.[0]?.uri,
    });
    return picked?.[0]?.fsPath ?? null;
  }

  private async connect(params: Record<string, unknown>) {
    const request = params.request as { carrier: string };
    const blocker = this.guard.blockedBy();
    if (request.carrier === "probe" && blocker) {
      throw new Error(`The debug session "${blocker}" is using the probe; stop it first, or connect over USB.`);
    }
    this.session = { id: params.session as number, carrier: request.carrier, live: true };
    this.releasedFor = null;
    if (this.notice) {
      this.notice = null;
      this.postStatus();
    }
    try {
      return await this.serverCall("session_connect", params);
    } catch (e) {
      if (this.session?.id === params.session) this.session.live = false;
      throw e;
    }
  }

  // studio-server

  private startServer(): StudioServer {
    const path = findServer(this.context.extensionPath);
    if (!path) {
      throw new Error(
        "studio-server not found. Build it (cargo build -p studio-server) or set tuningStudio.serverPath.",
      );
    }
    const args = vscode.workspace.getConfiguration("tuningStudio").get<boolean>("mockTarget") ? ["--mock"] : [];
    this.log.appendLine(`starting ${path} ${args.join(" ")}`);
    const server = new StudioServer(path, args, {
      event: (session, event) => {
        if (this.session?.id === session && event.type === "status") {
          this.session.live = event.state === "connecting" || event.state === "connected";
        }
        void this.panel.webview.postMessage({ type: "event", session, event });
      },
      appEvent: (event) => {
        void this.panel.webview.postMessage({ type: "app_event", event });
      },
      frame: (session, tts1) => {
        void this.panel.webview.postMessage({ type: "frame", session, data: tts1 });
      },
      exit: (code, expected) => {
        if (this.server === server) this.server = null;
        if (expected) return;
        this.log.appendLine(`studio-server exited (code ${code})`);
        void vscode.window.showErrorMessage(`Tuning Studio: studio-server stopped (code ${code}). Reopen the ELF to go on.`);
        if (this.session) {
          const event = { type: "status", state: "failed", message: "studio-server stopped" };
          void this.panel.webview.postMessage({ type: "event", session: this.session.id, event });
          this.session.live = false;
        }
      },
      log: (line) => this.log.appendLine(line),
    });
    server.ready.then((r) => this.log.appendLine(`studio-server ${r.version} ready${r.mock ? " (mock target)" : ""}`));
    this.server = server;
    return server;
  }

  private serverCall(method: string, params: Record<string, unknown>) {
    return (this.server ?? this.startServer()).call(method, params);
  }

  private html(media: vscode.Uri): string {
    const index = join(media.fsPath, "index.html");
    if (!existsSync(index)) {
      return `<!doctype html><body style="font-family: var(--vscode-font-family); padding: 1em">
        <p>The Tuning Studio page is not built. Run <code>npm run build:webview</code> in the repository, then reopen.</p></body>`;
    }
    const webview = this.panel.webview;
    const nonce = randomBytes(16).toString("base64");
    const csp = [
      "default-src 'none'",
      `img-src ${webview.cspSource} data:`,
      `font-src ${webview.cspSource}`,
      `style-src ${webview.cspSource} 'unsafe-inline'`,
      `script-src 'nonce-${nonce}'`,
    ].join("; ");
    const storage = this.context.workspaceState.get<Record<string, string>>(STORAGE_KEY) ?? {};
    // A JSON script is data, not code; `<` is escaped so no value can close the tag
    const boot = JSON.stringify({ storage }).replace(/</g, "\\u003c");
    return readFileSync(index, "utf8")
      .replace(/(src|href)="\.\/([^"]+)"/g, (_, attr: string, path: string) => {
        return `${attr}="${webview.asWebviewUri(vscode.Uri.joinPath(media, path))}"`;
      })
      .replace(/<script /g, `<script nonce="${nonce}" `)
      .replace(
        "<head>",
        `<head>\n    <meta http-equiv="Content-Security-Policy" content="${csp}" />` +
          `\n    <script id="tuning-studio-boot" type="application/json">${boot}</script>`,
      );
  }

  private dispose() {
    if (StudioPanel.current === this) StudioPanel.current = undefined;
    this.disposables.forEach((d) => d.dispose());
    // Closing stdin stops the session, so the probe is released before the server exits
    void this.server?.dispose();
    this.server = null;
  }
}

