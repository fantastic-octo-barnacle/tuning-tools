import * as vscode from "vscode";
import { StudioPanel, VIEW_TYPE } from "./panel";
import { ProbeGuard } from "./probeGuard";
import { StudioSession } from "./session";
import { registerWorkbench } from "./workbench";

export function activate(context: vscode.ExtensionContext) {
  const log = vscode.window.createOutputChannel("Tuning Studio");
  const firmwareLog = vscode.window.createOutputChannel("Tuning Studio: Firmware");
  const guard = new ProbeGuard(log);
  const session = new StudioSession(context, guard, log, firmwareLog);
  context.subscriptions.push(log, firmwareLog, guard, session,
    vscode.window.registerWebviewPanelSerializer(VIEW_TYPE, {
      async deserializeWebviewPanel(panel) { StudioPanel.revive(panel, context, session); },
    }),
  );
  registerWorkbench(context, session, () => StudioPanel.show(context, session));
}

export function deactivate() {}
