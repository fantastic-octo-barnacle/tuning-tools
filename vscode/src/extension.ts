import * as vscode from "vscode";
import { StudioPanel, VIEW_TYPE } from "./panel";
import { ProbeGuard } from "./probeGuard";

export function activate(context: vscode.ExtensionContext) {
  const log = vscode.window.createOutputChannel("Tuning Studio");
  const guard = new ProbeGuard(log);
  context.subscriptions.push(
    log,
    guard,
    vscode.commands.registerCommand("tuningStudio.open", () => StudioPanel.show(context, guard, log)),
    vscode.window.registerWebviewPanelSerializer(VIEW_TYPE, {
      async deserializeWebviewPanel(panel) {
        StudioPanel.revive(panel, context, guard, log);
      },
    }),
  );
}

export function deactivate() {}
