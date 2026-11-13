/**
 * The playground's execution boundary.
 *
 * Saule compiles to WebAssembly from the same interpreter the CLI uses (see
 * `crates/saule-wasm`), and runs here inside a Web Worker. The worker is not
 * an optimisation — it is the only way to stop a running program, because a
 * wasm module cannot be interrupted from the outside. See `saule-worker.ts`.
 *
 * ## Two workers
 *
 * **Execution** gets its own, because stopping a program means terminating
 * the worker it runs in. **Analysis** — `check` for the editor's squiggles,
 * `format` for the Format button — gets a second one that is never killed.
 *
 * One worker would have meant the editor going quiet for as long as a slow
 * program ran, and Stop throwing away whatever check was in flight. The cost
 * is a second instance of the module; the browser fetches and caches the
 * bytes once, so what is paid twice is instantiation, not download.
 *
 * Both are lazy. Opening `/play/` spawns neither: the analysis worker starts
 * on the first keystroke or Format, the execution worker on the first Run.
 */

export type DiagnosticSeverity = 'error' | 'warning';

/** A compile-time or run-time diagnostic, positioned in the source. */
export interface Diagnostic {
	severity: DiagnosticSeverity;
	/** Which phase produced it — surfaced as a label in the output pane. */
	phase: 'lex' | 'parse' | 'semantic' | 'type' | 'compile' | 'runtime';
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

/** What `format` answers: the laid-out source, or why there is none. */
export interface FormatResult {
	ok: boolean;
	text?: string;
	diagnostic?: Diagnostic;
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
	resolve: (json: string) => void;
	reject: (reason: unknown) => void;
	timeout?: ReturnType<typeof setTimeout>;
}

/** Version string reported by the module once it has initialised. */
let runtimeVersion: string | null = null;
export function getRuntimeVersion(): string | null {
	return runtimeVersion;
}

let nextId = 1;

type Job = 'run' | 'check' | 'format' | 'complete';

/** A request waiting its turn. */
interface Queued {
	type: Job;
	source: string;
	timeoutMs?: number;
	/** The caret, for `complete`: a UTF-8 byte offset into `source`. */
	offset?: number;
	resolve: (json: string) => void;
	reject: (reason: unknown) => void;
}

/**
 * One worker, serving one request at a time.
 *
 * The module is single-threaded, so requests queue rather than overlap.
 * Queuing here rather than rejecting is what lets Format and the editor's
 * checks share a worker without either having to know the other exists.
 */
class Channel {
	private worker: Worker | null = null;
	private pending: Pending | null = null;
	private queue: Queued[] = [];
	private currentId = 0;

	get busy(): boolean {
		return this.pending !== null;
	}

	/** Send one request, resolving with the JSON the module returned. */
	send(type: Job, source: string, timeoutMs?: number, offset?: number): Promise<string> {
		return new Promise<string>((resolve, reject) => {
			this.queue.push({ type, source, timeoutMs, offset, resolve, reject });
			this.pump();
		});
	}

	/** Start the next queued request, if the worker is free. */
	private pump() {
		if (this.pending) return;
		const next = this.queue.shift();
		if (!next) return;

		if (!this.worker) {
			try {
				this.worker = this.spawn();
			} catch (err) {
				next.reject(
					new RuntimeUnavailableError(
						`Could not start the Saule runtime: ${describe(err)}`
					)
				);
				this.pump();
				return;
			}
		}

		const id = nextId++;
		this.currentId = id;
		const timeout =
			next.timeoutMs === undefined
				? undefined
				: setTimeout(() => {
						this.cancel(
							new RunCancelledError(
								`The program ran for more than ${
									next.timeoutMs! / 1000
								} seconds and was stopped. An infinite loop, perhaps?`
							)
						);
					}, next.timeoutMs);

		this.pending = { resolve: next.resolve, reject: next.reject, timeout };
		this.worker.postMessage({ type: next.type, id, source: next.source, offset: next.offset });
	}

	/**
	 * Abandon whatever is in flight and discard the worker.
	 *
	 * Terminating is the only reliable way: a wasm module in a tight loop
	 * never yields, so there is nothing to politely ask. The next request
	 * spawns a fresh worker, which also re-initialises the module — a fair
	 * price for being able to escape an infinite loop at all.
	 */
	cancel(reason: unknown): void {
		const settle = this.pending;
		this.pending = null;
		if (settle) {
			clearTimeout(settle.timeout);
			settle.reject(reason);
		}
		// Anything queued behind it was queued against a worker that no
		// longer exists, but the request itself is still valid — a fresh
		// worker will serve it.
		this.dispose();
		this.pump();
	}

	private dispose() {
		if (this.worker) {
			this.worker.terminate();
			this.worker = null;
		}
	}

	private spawn(): Worker {
		// `new URL(..., import.meta.url)` is the form Vite recognises, so the
		// worker and the wasm asset are bundled and hashed like everything
		// else.
		const w = new Worker(new URL('./saule-worker.ts', import.meta.url), { type: 'module' });

		w.addEventListener('message', (event: MessageEvent<WorkerResponse>) => {
			const message = event.data;

			if (message.type === 'ready') {
				runtimeVersion = message.version;
				return;
			}

			// Ignore replies from a request that was already cancelled — its
			// worker was terminated, but a queued message could still arrive.
			if (!this.pending || message.id !== this.currentId) return;

			const settle = this.pending;
			this.pending = null;
			clearTimeout(settle.timeout);

			if (message.type === 'error') {
				settle.reject(new RuntimeUnavailableError(message.message));
				// A failed module poisons this worker; the next request gets
				// a new one.
				this.dispose();
			} else {
				settle.resolve(message.json);
			}
			this.pump();
		});

		w.addEventListener('error', (event) => {
			this.cancel(
				new RuntimeUnavailableError(event.message || 'The Saule runtime failed to start.')
			);
		});

		return w;
	}
}

function describe(err: unknown): string {
	return err instanceof Error ? err.message : String(err);
}

const execution = new Channel();
const analysis = new Channel();

/** True while a program is executing. */
export function isRunning(): boolean {
	return execution.busy;
}

/** Stop the running program. */
export function stopSaule(reason = 'Stopped.'): void {
	execution.cancel(new RunCancelledError(reason));
}

/** Compile and run a Saule program, returning its output and diagnostics. */
export async function runSaule(source: string): Promise<RunResult> {
	const startedAt = performance.now();
	const json = await execution.send('run', source, RUN_TIMEOUT_MS);
	let parsed: Omit<RunResult, 'durationMs'>;
	try {
		parsed = JSON.parse(json) as Omit<RunResult, 'durationMs'>;
	} catch (err) {
		throw new RuntimeUnavailableError(
			`The runtime returned a malformed result: ${describe(err)}`
		);
	}
	return { ...parsed, durationMs: performance.now() - startedAt };
}

/**
 * Every compile-time diagnostic in `source`, without running it.
 *
 * What the editor's squiggles are drawn from. Superseded requests are the
 * normal case — someone types faster than the module answers — so each call
 * takes a ticket, and a call whose ticket is no longer the newest resolves
 * to `null` rather than to diagnostics for text that has already changed.
 * Drawing a stale answer is worse than drawing none: it underlines a
 * character that has since moved.
 */
export async function checkSaule(source: string): Promise<Diagnostic[] | null> {
	const ticket = ++checkTicket;
	const json = await analysis.send('check', source);
	if (ticket !== checkTicket) return null;
	const parsed = JSON.parse(json) as { diagnostics: Diagnostic[] };
	return parsed.diagnostics;
}

let checkTicket = 0;

/** One suggestion from the language server's completion engine. */
export interface SauleCompletion {
	label: string;
	/** CodeMirror's completion type: `method`, `variable`, `class`, … */
	kind: string;
	detail?: string;
	/** Markdown documentation, when the item has any. */
	doc?: string;
}

/**
 * Completions at `offset` (a UTF-8 byte offset), ranked best first.
 *
 * The same engine the editor plugins use, run in the analysis worker: it
 * parses the buffer, so it knows what is in scope and what type a receiver
 * has. A request overtaken by a newer one resolves to `null`.
 */
export async function completeSaule(
	source: string,
	offset: number
): Promise<SauleCompletion[] | null> {
	const ticket = ++completeTicket;
	const json = await analysis.send('complete', source, undefined, offset);
	if (ticket !== completeTicket) return null;
	return JSON.parse(json) as SauleCompletion[];
}

let completeTicket = 0;

/** Pretty-print `source` — the same pass `saule fmt` runs. */
export async function formatSaule(source: string): Promise<FormatResult> {
	const json = await analysis.send('format', source);
	return JSON.parse(json) as FormatResult;
}
