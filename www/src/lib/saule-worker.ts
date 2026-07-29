/// <reference lib="webworker" />
/**
 * Runs Saule programs off the main thread.
 *
 * A WebAssembly module cannot be interrupted from the outside — there is no
 * way to ask it to stop mid-execution. So `while true do end`, which is an
 * easy thing to type by accident in a playground, would wedge the page
 * permanently if it ran on the main thread: no repaint, no Stop button, no
 * way back except closing the tab.
 *
 * Running it in a worker makes it terminable. The page stays responsive
 * whatever the program does, and `worker.terminate()` is an unconditional
 * kill. That is the entire reason this file exists.
 *
 * Protocol — the main thread sends `{ type: 'run', id, source }`, and gets
 * back exactly one `{ type: 'result' | 'error', id, ... }` per request. A
 * `{ type: 'ready' }` is posted once the wasm module has initialised, so the
 * page can show that the runtime is warming up on first use.
 */

import init, { run, version } from './saule_wasm/saule_wasm.js';
// Vite rewrites this to the emitted asset URL (hashed, and prefixed with the
// site's base path), so the module is fetched rather than inlined as base64 —
// which keeps it cacheable and out of the JS bundle.
import wasmUrl from './saule_wasm/saule_wasm_bg.wasm?url';

export type WorkerRequest = { type: 'run'; id: number; source: string };

export type WorkerResponse =
	| { type: 'ready'; version: string }
	| { type: 'result'; id: number; json: string }
	| { type: 'error'; id: number; message: string };

const ctx = self as unknown as DedicatedWorkerGlobalScope;

/** Initialise once; every later run awaits the same promise. */
const ready: Promise<void> = init({ module_or_path: wasmUrl }).then(() => {
	const message: WorkerResponse = { type: 'ready', version: version() };
	ctx.postMessage(message);
});

ctx.addEventListener('message', (event: MessageEvent<WorkerRequest>) => {
	const request = event.data;
	if (request?.type !== 'run') return;

	void ready
		.then(() => {
			// `run` returns the JSON string described by `RunResult` in
			// runtime.ts. Parsing happens on the main thread so a malformed
			// payload surfaces there rather than as a dead worker.
			const json = run(request.source);
			const message: WorkerResponse = { type: 'result', id: request.id, json };
			ctx.postMessage(message);
		})
		.catch((err: unknown) => {
			// A panic inside the module traps it, and every later call would
			// fail too. Reporting it is better than going silent; the main
			// thread discards this worker and starts a fresh one.
			const message: WorkerResponse = {
				type: 'error',
				id: request.id,
				message: err instanceof Error ? err.message : String(err),
			};
			ctx.postMessage(message);
		});
});
