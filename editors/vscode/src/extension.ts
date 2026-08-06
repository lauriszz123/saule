// Saule VS Code extension entry point.
//
// Launches the `saule-lsp` language server and wires VS Code up to it via the
// Language Server Protocol. The server provides diagnostics, hover,
// go-to-definition, references, document highlights/symbols, inlay hints,
// signature help, and formatting — see `crates/saule-lsp`.
//
// On top of the server, and matching the IntelliJ plugin feature for feature:
//
//   * indentation while typing, including the dedent of block-closing keywords
//     (`./reindent.ts`, `./indent.ts`);
//   * toolchain discovery that walks up to the Cargo build output
//     (`./toolchain.ts`);
//   * run commands for a single file and for the whole project, the equivalent
//     of the plugin's two run configurations.

import {
  ExtensionContext,
  LogOutputChannel,
  Terminal,
  Uri,
  commands,
  languages,
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

import { SauleOnTypeFormatting, TRIGGER_CHARACTERS } from "./reindent";
import { registerSignatureHelpFollowsCaret } from "./signature";
import { Located, locate } from "./toolchain";

let client: LanguageClient | undefined;
let outputChannel: LogOutputChannel | undefined;
let terminal: Terminal | undefined;

const SAULE_SELECTOR = { scheme: "file", language: "saule" } as const;

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
    commands.registerCommand("saule.runFile", () => runFile()),
    commands.registerCommand("saule.runProject", () => runProject()),
  );

  // Indentation while typing — the counterpart of the plugin's enter handler
  // and auto-dedent. Registered client-side because `saule-lsp` advertises no
  // `documentOnTypeFormattingProvider`, so nothing else claims these keys.
  context.subscriptions.push(
    languages.registerOnTypeFormattingEditProvider(
      SAULE_SELECTOR,
      new SauleOnTypeFormatting(),
      "\n",
      ...TRIGGER_CHARACTERS,
    ),
  );

  // Parameter info follows the caret: the popup appears whenever the caret is
  // inside a call's parens, not only when `(` is typed.
  context.subscriptions.push(registerSignatureHelpFollowsCaret());

  context.subscriptions.push(
    window.onDidCloseTerminal((closed) => {
      if (closed === terminal) terminal = undefined;
    }),
  );

  // Restart automatically when the server path / args change.
  context.subscriptions.push(
    workspace.onDidChangeConfiguration((e) => {
      if (
        e.affectsConfiguration("saule.server.path") ||
        e.affectsConfiguration("saule.server.extraArgs") ||
        e.affectsConfiguration("saule.toolchainDir")
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
  const server = locate("saule-lsp", "SAULE_LSP_PATH", "server.path");
  if (!server.found) {
    const choice = await window.showWarningMessage(
      "Saule language server (`saule-lsp`) not found. Syntax highlighting and " +
        "indentation are active, but diagnostics, hover, and navigation are " +
        "disabled. Build it with `cargo build --release -p saule-lsp`, or set " +
        "`saule.server.path`.",
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
    command: server.command,
    args: extraArgs,
    transport: TransportKind.stdio,
    // Launched from the directory holding the build output, so the server
    // resolves project files the same way the IntelliJ plugin's does.
    options: { env: process.env, cwd: server.workingDir },
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
    outputChannel?.appendLine(
      `saule-lsp started: ${server.command} (cwd=${server.workingDir ?? "-"})`,
    );
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

// ── running ─────────────────────────────────────────────────────────────────

/** `saule run <file>` — the plugin's single-file run configuration. */
async function runFile(): Promise<void> {
  const document = window.activeTextEditor?.document;
  if (!document || document.languageId !== "saule") {
    window.showErrorMessage("Saule: no .sau file is active.");
    return;
  }
  if (document.isDirty) {
    await document.save();
  }
  await run([shellQuote(document.uri.fsPath)], document.uri);
}

/** `saule run` — the plugin's project run configuration. */
async function runProject(): Promise<void> {
  await run([], window.activeTextEditor?.document.uri);
}

async function run(args: string[], context: Uri | undefined): Promise<void> {
  const cli: Located = locate("saule", "SAULE_PATH", "cli.path");
  if (!cli.found) {
    const choice = await window.showErrorMessage(
      "Could not find the 'saule' executable. Build it with `cargo build " +
        "--release`, set `saule.cli.path`, or add 'saule' to your PATH.",
      "Open Settings",
    );
    if (choice === "Open Settings") {
      await commands.executeCommand(
        "workbench.action.openSettings",
        "saule.cli.path",
      );
    }
    return;
  }

  const cwd =
    (context ? workspace.getWorkspaceFolder(context)?.uri.fsPath : undefined) ??
    cli.workingDir;

  if (!terminal || terminal.exitStatus !== undefined) {
    terminal = window.createTerminal({ name: "Saule", cwd });
  }
  terminal.show(true);
  terminal.sendText([shellQuote(cli.command), "run", ...args].join(" "));
}

/** Quote a path for the shell the integrated terminal is running. */
function shellQuote(value: string): string {
  if (!/[\s"'$`\\]/.test(value)) return value;
  return process.platform === "win32"
    ? `"${value}"`
    : `'${value.replace(/'/g, `'\\''`)}'`;
}
