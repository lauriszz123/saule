/**
 * The playground's execution boundary.
 *
 * Saule compiles to WebAssembly from the same interpreter the CLI uses (see
 * `crates/saule-wasm`), and runs here inside a Web Worker. The worker is not
 * an optimisation — it is the only way to stop a running program, because a
 * wasm module cannot be interrupted from the outside. See `saule-worker.ts`.
 *
 * The module is ~1.1 MB (~333 KB gzipped) and is fetched lazily: the worker
 * is not spawned until the first run, so opening `/play/` costs nothing extra
 * until someone presses Run.
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

/** Thrown when a run is cancelled — by the user, or by the timeout. */
export class RunCancelledError extends Error {
	constructor(message: string) {
		super(message);
		this.name = 'RunCancelledError';
	}
}

/**
 * Whether a Saule runtime is available in this build.
 *
 * Kept as an export because the playground reads it on mount. It is now
 * always true: if the wasm module were missing the site would not build, so
 * there is no half-working state to guard against.
 */
export const RUNTIME_AVAILABLE = true;

/**
 * How long a program may run before it is killed automatically.
 *
 * A backstop for someone who writes an infinite loop and wanders off, not a
 * performance budget — the Stop button is the primary control. Generous
 * enough that a legitimately slow program is not cut short.
 */
export const RUN_TIMEOUT_MS = 10_000;

type WorkerResponse =
	| { type: 'ready'; version: string }
	| { type: 'result'; id: number; json: string }
	| { type: 'error'; id: number; message: string };

interface Pending {
	resolve: (value: RunResult) => void;
	reject: (reason: unknown) => void;
	startedAt: number;
	timeout: ReturnType<typeof setTimeout>;
}

let worker: Worker | null = null;
let pending: Pending | null = null;
let nextId = 1;

/** Version string reported by the module once it has initialised. */
let runtimeVersion: string | null = null;
export function getRuntimeVersion(): string | null {
	return runtimeVersion;
}

/** True while a program is executing. */
export function isRunning(): boolean {
	return pending !== null;
}

function spawnWorker(): Worker {
	// `new URL(..., import.meta.url)` is the form Vite recognises, so the
	// worker and the wasm asset are bundled and hashed like everything else.
	const w = new Worker(new URL('./saule-worker.ts', import.meta.url), {
		type: 'module',
	});

	w.addEventListener('message', (event: MessageEvent<WorkerResponse>) => {
		const message = event.data;

		if (message.type === 'ready') {
			runtimeVersion = message.version;
			return;
		}

		// Ignore replies from a run that was already cancelled — its worker
		// was terminated, but a queued message could still arrive.
		if (!pending || message.id !== currentId) return;

		const settle = pending;
		pending = null;
		clearTimeout(settle.timeout);

		if (message.type === 'error') {
			settle.reject(new RuntimeUnavailableError(message.message));
			// A failed module poisons this worker; the next run gets a new one.
			disposeWorker();
			return;
		}

		try {
			const parsed = JSON.parse(message.json) as Omit<RunResult, 'durationMs'>;
			settle.resolve({
				...parsed,
				durationMs: performance.now() - settle.startedAt,
			});
		} catch (err) {
			settle.reject(
				new RuntimeUnavailableError(
					`The runtime returned a malformed result: ${
						err instanceof Error ? err.message : String(err)
					}`
				)
			);
		}
	});

	w.addEventListener('error', (event) => {
		const settle = pending;
		pending = null;
		if (settle) {
			clearTimeout(settle.timeout);
			settle.reject(
				new RuntimeUnavailableError(
					event.message || 'The Saule runtime failed to start.'
				)
			);
		}
		disposeWorker();
	});

	return w;
}

function disposeWorker() {
	if (worker) {
		worker.terminate();
		worker = null;
	}
}

let currentId = 0;

/**
 * Stop the running program.
 *
 * Terminating the worker is the only reliable way: a wasm module in a tight
 * loop never yields, so there is nothing to politely ask. The next run spawns
 * a fresh worker, which also means the ~1.1 MB module is re-initialised — a
 * fair price for being able to escape an infinite loop at all.
 */
export function stopSaule(reason = 'Stopped.'): void {
	const settle = pending;
	pending = null;
	if (settle) {
		clearTimeout(settle.timeout);
		settle.reject(new RunCancelledError(reason));
	}
	disposeWorker();
}

/** Compile and run a Saule program, returning its output and diagnostics. */
export function runSaule(source: string): Promise<RunResult> {
	if (pending) {
		return Promise.reject(new Error('A program is already running.'));
	}

	if (!worker) {
		try {
			worker = spawnWorker();
		} catch (err) {
			return Promise.reject(
				new RuntimeUnavailableError(
					`Could not start the Saule runtime: ${
						err instanceof Error ? err.message : String(err)
					}`
				)
			);
		}
	}

	const id = nextId++;
	currentId = id;

	return new Promise<RunResult>((resolve, reject) => {
		const timeout = setTimeout(() => {
			stopSaule(
				`The program ran for more than ${
					RUN_TIMEOUT_MS / 1000
				} seconds and was stopped. An infinite loop, perhaps?`
			);
		}, RUN_TIMEOUT_MS);

		pending = { resolve, reject, startedAt: performance.now(), timeout };
		worker!.postMessage({ type: 'run', id, source });
	});
}
