/**
 * The playground's execution boundary.
 *
 * Everything the UI needs from the language lives behind `runSaule`. Today it
 * reports that the browser runtime isn't built yet; when `crates/saule-wasm`
 * lands, only the body of `load()` changes — the page, the editor and the
 * output pane all keep working against this same shape.
 *
 * The interface is deliberately the one a real run produces: a stream of
 * output chunks in emission order, plus structured diagnostics carrying byte
 * spans, so the editor can underline the offending range rather than printing
 * a bare message.
 */

export type DiagnosticSeverity = 'error' | 'warning';

/** A compile-time or run-time diagnostic, positioned in the source. */
export interface Diagnostic {
	severity: DiagnosticSeverity;
	/** Which phase produced it — surfaced as a label in the output pane. */
	phase: 'lex' | 'parse' | 'semantic' | 'type' | 'runtime';
	message: string;
	/** Byte offsets into the source. Absent for errors with no location. */
	span?: { start: number; end: number };
	/** miette's help text, when the diagnostic carries one. */
	help?: string;
}

export interface OutputChunk {
	stream: 'stdout' | 'stderr';
	text: string;
}

export interface RunResult {
	output: OutputChunk[];
	diagnostics: Diagnostic[];
	/** Wall-clock duration of the run, in milliseconds. */
	durationMs: number;
	/** True when the program ran to completion with no errors. */
	ok: boolean;
}

/** Thrown by `runSaule` when the runtime itself could not be loaded. */
export class RuntimeUnavailableError extends Error {
	constructor(message: string) {
		super(message);
		this.name = 'RuntimeUnavailableError';
	}
}

/**
 * Whether a Saule runtime is actually available in this build.
 *
 * The playground checks this on mount so it can explain itself up front
 * instead of letting someone write a program and only then discover that
 * Run does nothing.
 */
export const RUNTIME_AVAILABLE = false;

type WasmModule = {
	run: (source: string) => string;
};

let modulePromise: Promise<WasmModule> | null = null;

/**
 * Load (and memoize) the WebAssembly build of the interpreter.
 *
 * Phase two replaces the throw with:
 *
 *   const wasm = await import('./saule_wasm/saule_wasm.js');
 *   await wasm.default();
 *   return wasm;
 *
 * where `saule_wasm` is the `wasm-bindgen` output of `crates/saule-wasm`.
 * Because it's a dynamic import, the ~1 MB module stays out of the main
 * bundle and only downloads when someone actually opens the playground.
 */
function load(): Promise<WasmModule> {
	if (!modulePromise) {
		modulePromise = Promise.reject(
			new RuntimeUnavailableError(
				'The browser runtime is still being built. Saule compiles to ' +
					'WebAssembly from the same interpreter the CLI uses, and that ' +
					'work is in progress — until then, install the toolchain to run ' +
					'your code locally.'
			)
		);
		// Nothing awaits this rejection until someone presses Run, and an
		// unhandled rejection would otherwise be logged on page load.
		modulePromise.catch(() => {});
	}
	return modulePromise;
}

/** Compile and run a Saule program, returning its output and diagnostics. */
export async function runSaule(source: string): Promise<RunResult> {
	const wasm = await load();
	const started = performance.now();
	const raw = wasm.run(source);
	const parsed = JSON.parse(raw) as Omit<RunResult, 'durationMs'>;
	return { ...parsed, durationMs: performance.now() - started };
}
