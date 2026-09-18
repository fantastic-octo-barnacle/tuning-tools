// Defaults from the workspace's probe-rs-debug launch configurations: the chip, the probe, and the
// firmware ELF (`programBinary`).

import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, resolve } from "node:path";
import * as vscode from "vscode";

export interface LaunchDefaults {
  /** The configuration they came from */
  name: string;
  chip: string | null;
  probe: string | null;
  speedKhz: number | null;
  /** Absolute, and only when the file exists */
  elfPath: string | null;
}

interface ProbeRsConfig {
  type?: string;
  name?: string;
  cwd?: string;
  chip?: string;
  probe?: string;
  speed?: number;
  programBinary?: string;
  coreConfigs?: { coreIndex?: number; programBinary?: string }[];
}

function substitute(value: string, folder: vscode.WorkspaceFolder): string {
  return value
    .replace(/\$\{workspaceFolder\}/g, folder.uri.fsPath)
    .replace(/\$\{workspaceFolderBasename\}/g, folder.name)
    .replace(/\$\{userHome\}/g, homedir())
    .replace(/\$\{env:([^}]+)\}/g, (_, name: string) => process.env[name] ?? "");
}

function fromConfig(config: ProbeRsConfig, folder: vscode.WorkspaceFolder): LaunchDefaults {
  const cwd = config.cwd ? substitute(config.cwd, folder) : folder.uri.fsPath;
  const core = config.coreConfigs?.find((c) => (c.coreIndex ?? 0) === 0) ?? config.coreConfigs?.[0];
  const binary = core?.programBinary ?? config.programBinary;
  let elfPath: string | null = null;
  const path = binary ? substitute(binary, folder) : null;
  // A variable left unresolved (`${command:...}`) names nothing usable
  if (path && !path.includes("${")) {
    const full = isAbsolute(path) ? path : resolve(cwd, path);
    if (existsSync(full)) elfPath = full;
  }
  return {
    name: config.name ?? "probe-rs-debug",
    chip: config.chip?.trim() || null,
    probe: config.probe?.trim() || null,
    speedKhz: typeof config.speed === "number" ? config.speed : null,
    elfPath,
  };
}

/** The first probe-rs-debug configuration in the workspace, preferring one whose ELF exists */
export function launchDefaults(): LaunchDefaults | null {
  const found: LaunchDefaults[] = [];
  for (const folder of vscode.workspace.workspaceFolders ?? []) {
    const configs = vscode.workspace.getConfiguration("launch", folder.uri).get<ProbeRsConfig[]>("configurations") ?? [];
    for (const config of configs) {
      if (config?.type === "probe-rs-debug") found.push(fromConfig(config, folder));
    }
  }
  return found.find((d) => d.elfPath) ?? found[0] ?? null;
}
