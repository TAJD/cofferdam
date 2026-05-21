// cofferdam VS Code language client — cd-9hp.4 cp5 smoke-test stub.
//
// Not published to the marketplace. Spawns the cofferdam LSP server
// over stdio, surfaces its diagnostics in the editor's Problems panel.
// To exercise: `code --extensionDevelopmentPath=editors/vscode`.

import {
  ExtensionContext,
  workspace,
  WorkspaceConfiguration,
} from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export function activate(context: ExtensionContext): void {
  const config: WorkspaceConfiguration = workspace.getConfiguration("cofferdam");
  const executable: string = config.get<string>("executable", "cofferdam");

  // The server is spawned as a child process. `cofferdam lsp` reads
  // LSP messages on stdin and writes them to stdout — TransportKind.stdio.
  const serverOptions: ServerOptions = {
    run: {
      command: executable,
      args: ["lsp"],
      transport: TransportKind.stdio,
    },
    debug: {
      command: executable,
      args: ["lsp"],
      transport: TransportKind.stdio,
    },
  };

  // Pull diagnostics for TypeScript / TSX files only. The server's
  // workspace scan covers everything else regardless of which docs
  // are open in the editor.
  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: "file", language: "typescript" },
      { scheme: "file", language: "typescriptreact" },
    ],
    synchronize: {
      // Re-read config when cofferdam.toml changes. The server itself
      // does not reload config in cp5 — restart for now — but the
      // client will surface the file-change events for follow-up beads.
      fileEvents: workspace.createFileSystemWatcher("**/cofferdam.toml"),
    },
  };

  client = new LanguageClient(
    "cofferdam",
    "cofferdam",
    serverOptions,
    clientOptions,
  );

  context.subscriptions.push({
    dispose: () => {
      void client?.stop();
    },
  });

  void client.start();
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}
