const { LanguageClient, TransportKind } = require("vscode-languageclient/node");
const vscode = require("vscode");
const path = require("path");

let client;

async function activate(context) {
  // Allow user to override the binary path via settings, otherwise
  // fall back to the cargo-built debug binary next to this extension.
  const config = vscode.workspace.getConfiguration("statix-lsp");
  const customPath = config.get("serverPath");

  const serverModule = customPath
    ? customPath
    : path.join(context.extensionPath, "target", "debug", "statix-lsp");

  const serverOptions = {
    run: { command: serverModule, transport: TransportKind.stdio },
    debug: { command: serverModule, transport: TransportKind.stdio },
  };

  const clientOptions = {
    documentSelector: [{ scheme: "file", language: "nix" }],
  };

  client = new LanguageClient(
    "statix-lsp",
    "Statix LSP",
    serverOptions,
    clientOptions,
  );

  await client.start();
  context.subscriptions.push(client);
}

async function deactivate() {
  if (client) {
    return client.stop();
  }
}

module.exports = {
  activate,
  deactivate,
};
