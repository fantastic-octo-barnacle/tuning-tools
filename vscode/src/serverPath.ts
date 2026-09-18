// Where studio-server is: the `tuningStudio.serverPath` setting, the binary bundled in the
// extension, or the newest cargo build in the workspace or next to the extension (development).

import { existsSync, statSync } from "node:fs";
import { isAbsolute, join, resolve } from "node:path";
import * as vscode from "vscode";

const EXE = process.platform === "win32" ? "studio-server.exe" : "studio-server";

export function findServer(extensionPath: string): string | null {
  const setting = vscode.workspace.getConfiguration("tuningStudio").get<string>("serverPath")?.trim();
  const folders = (vscode.workspace.workspaceFolders ?? []).map((f) => f.uri.fsPath);
  if (setting) {
    const expanded = setting.replace(/\$\{workspaceFolder\}/g, folders[0] ?? "");
    const path = isAbsolute(expanded) ? expanded : resolve(folders[0] ?? extensionPath, expanded);
    return existsSync(path) ? path : null;
  }
  const bundled = join(extensionPath, "bin", EXE);
  if (existsSync(bundled)) return bundled;
  const builds = [...folders, resolve(extensionPath, "..")]
    .flatMap((root) => ["release", "debug"].map((profile) => join(root, "target", profile, EXE)))
    .filter(existsSync)
    .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs);
  return builds[0] ?? null;
}
