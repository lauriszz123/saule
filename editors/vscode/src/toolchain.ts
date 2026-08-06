// Locates the Saule toolchain binaries (`saule`, `saule-lsp`) and the
// workspace root to run them from.
//
// A port of the IntelliJ plugin's `SauleToolchain` (see
// `editors/intellij/.../SauleToolchain.kt`), with the same resolution order so
// both editors pick the same binary in the same project:
//
//   1. Environment variable override (`SAULE_LSP_PATH` / `SAULE_PATH`).
//   2. The configured path or toolchain directory in Settings.
//   3. Cargo build output, walking UP from each workspace folder looking for
//      `target/{release,debug}/<name>`. Walking up is what lets you open a
//      sub-folder (say `examples/todo-app`) and still find the workspace-root
//      build output — and the directory holding that `target/` becomes the
//      working directory the binary is launched in.
//   4. `PATH`.
//   5. The bare name, with `found: false`, so callers can surface a hint.

import * as fs from "fs";
import * as path from "path";
import { workspace } from "vscode";

/** How far up the tree to look for a `target/` directory. */
const MAX_DEPTH = 24;

export interface Located {
  /** The executable path, or the bare name if nothing was found. */
  readonly command: string;
  /** Where to run it (the directory containing `target/`, when known). */
  readonly workingDir: string | undefined;
  /** True if [command] resolved to a real file or PATH entry. */
  readonly found: boolean;
}

/** Windows needs the `.exe` suffix; Unix does not. */
export function exeName(base: string): string {
  return process.platform === "win32" ? `${base}.exe` : base;
}

/**
 * @param base       bare binary name, e.g. `"saule"` or `"saule-lsp"`.
 * @param envVar     environment variable that, if set, pins the executable.
 * @param setting    `saule.`-scoped setting holding an explicit path.
 */
export function locate(
  base: string,
  envVar: string,
  setting: string,
): Located {
  const exe = exeName(base);
  const config = workspace.getConfiguration("saule");
  const firstFolder = workspace.workspaceFolders?.[0]?.uri.fsPath;

  // 1. Environment override.
  const fromEnv = process.env[envVar]?.trim();
  if (fromEnv) {
    return { command: fromEnv, workingDir: firstFolder, found: true };
  }

  // 2. Configured path, then configured toolchain directory.
  const configured = config.get<string>(setting, "").trim();
  if (configured) {
    return {
      command: configured,
      workingDir: firstFolder,
      found: isFile(configured),
    };
  }
  const toolchainDir = config.get<string>("toolchainDir", "").trim();
  if (toolchainDir) {
    const candidate = path.join(toolchainDir, exe);
    if (isFile(candidate)) {
      return { command: candidate, workingDir: firstFolder, found: true };
    }
  }

  // 3. Cargo build output, walking up from every workspace folder.
  for (const folder of workspace.workspaceFolders ?? []) {
    const hit = findUpward(folder.uri.fsPath, exe);
    if (hit) {
      return { command: hit.command, workingDir: hit.workspaceRoot, found: true };
    }
  }

  // 4. PATH.
  const onPath = findOnPath(exe);
  if (onPath) {
    return { command: onPath, workingDir: firstFolder, found: true };
  }

  // 5. Nothing found.
  return { command: exe, workingDir: firstFolder, found: false };
}

/**
 * Walk up from `start`, returning the `target/{release,debug}/exe` and the
 * directory that contains that `target/` folder (the workspace root).
 */
function findUpward(
  start: string,
  exe: string,
): { command: string; workspaceRoot: string } | undefined {
  let dir = start;
  for (let depth = 0; depth < MAX_DEPTH; depth++) {
    for (const profile of ["release", "debug"]) {
      const candidate = path.join(dir, "target", profile, exe);
      if (isFile(candidate)) {
        return { command: candidate, workspaceRoot: dir };
      }
    }
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return undefined;
}

function findOnPath(exe: string): string | undefined {
  const raw = process.env.PATH;
  if (!raw) return undefined;
  for (const entry of raw.split(path.delimiter)) {
    if (!entry.trim()) continue;
    const candidate = path.join(entry, exe);
    if (isFile(candidate)) return candidate;
  }
  return undefined;
}

function isFile(candidate: string): boolean {
  try {
    return fs.statSync(candidate).isFile();
  } catch {
    return false;
  }
}
