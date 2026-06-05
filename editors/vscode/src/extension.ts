// Saule VS Code extension entry point.
//
// Launches the `saule-lsp` language server and wires VS Code up to it via
// the Language Server Protocol. The server provides diagnostics, hover,
// go-to-definition, references, document highlights/symbols, inlay hints,
// signature help, and formatting — see `crates/saule-lsp`.
//
// Server-binary resolution order:
//   1. `saule.server.path` setting (if non-empty).
//   2. `<workspaceFolder>/target/{release,debug}/saule-lsp[.exe]`, for the
//      common case of developing inside the Saule repo itself.
//   3. `saule-lsp[.exe]` on `$PATH`.

import * as fs from "fs";
import * as path from "path";
import {
  ExtensionContext,
  LogOutputChannel,
  commands,
  window,
  workspace,
} from "vscode";
import {
  Executable,
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;
let outputChannel: LogOutputChannel | undefined;

const SERVER_EXE = process.platform === "win32" ? "saule-lsp.exe" : "saule-lsp";

export async function activate(context: ExtensionContext): Promise<void> {
  outputChannel = window.createOutputChannel("Saule Language Server", {
    log: true,
  });
  context.subscriptions.push(outputChannel);

  context.subscriptions.push(
    commands.registerCommand("saule.restartServer", async () => {
      await restart(context);
    }),
    commands.registerCommand("saule.showServerOutput", () => {
      outputChannel?.show(true);
    }),
  );

  // Restart automatically when the server path / args change.
  context.subscriptions.push(
    workspace.onDidChangeConfiguration((e) => {
      if (
        e.affectsConfiguration("saule.server.path") ||
        e.affectsConfiguration("saule.server.extraArgs")
      ) {
        void restart(context);
      }
    }),
  );

  await start(context);
}

export async function deactivate(): Promise<void> {
  await stopClient();
}

async function start(context: ExtensionContext): Promise<void> {
  const serverPath = resolveServerPath();
  if (!serverPath) {
    const choice = await window.showWarningMessage(
      "Saule language server (`saule-lsp`) not found. Syntax highlighting is " +
        "active, but diagnostics, hover, and navigation are disabled. Build it " +
        "with `cargo build --release -p saule-lsp`, or set `saule.server.path`.",
      "Open Settings",
    );
    if (choice === "Open Settings") {
      await commands.executeCommand(
        "workbench.action.openSettings",
        "saule.server.path",
      );
    }
    return;
  }

  const extraArgs = workspace
    .getConfiguration("saule")
    .get<string[]>("server.extraArgs", []);

  const executable: Executable = {
    command: serverPath,
    args: extraArgs,
    transport: TransportKind.stdio,
    options: { env: process.env },
  };

  const serverOptions: ServerOptions = {
    run: executable,
    debug: executable,
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: "file", language: "saule" },
      { scheme: "untitled", language: "saule" },
    ],
    synchronize: {
      // Re-analyse when project config or sibling sources change on disk.
      fileEvents: workspace.createFileSystemWatcher("**/{*.sau,saule.config}"),
    },
    outputChannel,
    traceOutputChannel: outputChannel,
  };

  client = new LanguageClient(
    "saule",
    "Saule Language Server",
    serverOptions,
    clientOptions,
  );

  try {
    await client.start();
    outputChannel?.appendLine(`saule-lsp started: ${serverPath}`);
    context.subscriptions.push({ dispose: () => void stopClient() });
  } catch (err) {
    window.showErrorMessage(`Failed to start saule-lsp: ${err}`);
    outputChannel?.appendLine(`Failed to start saule-lsp: ${err}`);
    client = undefined;
  }
}

async function stopClient(): Promise<void> {
  if (client) {
    await client.stop();
    client = undefined;
  }
}

async function restart(context: ExtensionContext): Promise<void> {
  outputChannel?.appendLine("Restarting saule-lsp…");
  await stopClient();
  await start(context);
}

/**
 * Resolve the path to the saule-lsp executable.
 *
 * Honours the `saule.server.path` setting first, then probes each
 * workspace folder's `target/release` and `target/debug` directories
 * (the layout produced by `cargo build [--release]` inside the Saule
 * repo), and finally trusts `$PATH`.
 */
function resolveServerPath(): string | undefined {
  const configured = workspace
    .getConfiguration("saule")
    .get<string>("server.path", "")
    .trim();
  if (configured) {
    return fs.existsSync(configured) ? configured : undefined;
  }

  for (const folder of workspace.workspaceFolders ?? []) {
    const root = folder.uri.fsPath;
    const candidates = [
      path.join(root, "target", "release", SERVER_EXE),
      path.join(root, "target", "debug", SERVER_EXE),
    ];
    for (const candidate of candidates) {
      if (fs.existsSync(candidate)) {
        return candidate;
      }
    }
  }

  // Fall back to whatever is on PATH; LanguageClient surfaces a clear
  // spawn error if it isn't actually there.
  return SERVER_EXE;
}
