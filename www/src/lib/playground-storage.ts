/**
 * Persistence for playground scripts a visitor writes themselves.
 *
 * There is no server behind this site, so "my scripts" means `localStorage`
 * and nothing else: the scripts live in one browser, on one machine, and are
 * shared by copying a link rather than by an account. That is the whole
 * storage model — see `Playground.astro` for the UI built on top of it.
 *
 * Every entry point degrades to an in-memory list rather than throwing.
 * `localStorage` is unavailable in Safari's private mode and behind some
 * privacy settings, and a playground that refuses to open an editor because
 * it cannot remember files would be worse than one that forgets them.
 */

export interface UserScript {
	id: string;
	name: string;
	source: string;
	/** Epoch milliseconds; the picker lists most-recently-edited first. */
	updatedAt: number;
}

const KEY = 'saule.playground.scripts.v1';

/** Set once `localStorage` has thrown, so we stop retrying on every keystroke. */
let volatile: UserScript[] | null = null;

function readRaw(): UserScript[] {
	if (volatile) return volatile;
	try {
		const raw = window.localStorage.getItem(KEY);
		if (!raw) return [];
		const parsed = JSON.parse(raw);
		if (!Array.isArray(parsed)) return [];
		// Anything in storage is untrusted input — it may predate a schema
		// change, or have been edited by hand in devtools.
		return parsed.filter(isScript);
	} catch {
		volatile ??= [];
		return volatile;
	}
}

function isScript(value: unknown): value is UserScript {
	if (typeof value !== 'object' || value === null) return false;
	const s = value as Record<string, unknown>;
	return (
		typeof s.id === 'string' &&
		typeof s.name === 'string' &&
		typeof s.source === 'string' &&
		typeof s.updatedAt === 'number'
	);
}

function writeRaw(scripts: UserScript[]) {
	if (volatile) {
		volatile = scripts;
		return;
	}
	try {
		window.localStorage.setItem(KEY, JSON.stringify(scripts));
	} catch {
		// Quota exhausted, or storage denied. Keep the session working.
		volatile = scripts;
	}
}

/** Whether edits will survive a reload. Drives a one-line warning in the UI. */
export function isPersistent(): boolean {
	if (volatile) return false;
	try {
		window.localStorage.setItem(`${KEY}.probe`, '1');
		window.localStorage.removeItem(`${KEY}.probe`);
		return true;
	} catch {
		return false;
	}
}

/** All saved scripts, most recently edited first. */
export function listScripts(): UserScript[] {
	return readRaw().sort((a, b) => b.updatedAt - a.updatedAt);
}

export function getScript(id: string): UserScript | null {
	return readRaw().find((s) => s.id === id) ?? null;
}

function newId(): string {
	if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) return crypto.randomUUID();
	return `s-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

/**
 * Normalise a name into something that reads like a filename: the picker and
 * the editor's title bar both show it as one.
 */
export function normaliseName(name: string): string {
	const trimmed = name.trim().replace(/[\\/]+/g, '-').slice(0, 60);
	if (!trimmed) return 'untitled.sau';
	return /\.sau$/i.test(trimmed) ? trimmed : `${trimmed}.sau`;
}

/** `scratch.sau`, then `scratch-2.sau`, … so two "New file"s never collide. */
export function uniqueName(desired: string): string {
	const base = normaliseName(desired);
	const taken = new Set(readRaw().map((s) => s.name.toLowerCase()));
	if (!taken.has(base.toLowerCase())) return base;

	const stem = base.replace(/\.sau$/i, '');
	for (let n = 2; ; n += 1) {
		const candidate = `${stem}-${n}.sau`;
		if (!taken.has(candidate.toLowerCase())) return candidate;
	}
}

export function createScript(name: string, source: string): UserScript {
	const script: UserScript = {
		id: newId(),
		name: uniqueName(name),
		source,
		updatedAt: Date.now(),
	};
	writeRaw([script, ...readRaw()]);
	return script;
}

/** Save an edit. Returns the updated script, or null if it no longer exists. */
export function updateScript(id: string, source: string): UserScript | null {
	const scripts = readRaw();
	const script = scripts.find((s) => s.id === id);
	if (!script) return null;
	script.source = source;
	script.updatedAt = Date.now();
	writeRaw(scripts);
	return script;
}

export function renameScript(id: string, name: string): UserScript | null {
	const scripts = readRaw();
	const script = scripts.find((s) => s.id === id);
	if (!script) return null;
	const next = normaliseName(name);
	// Only de-duplicate against *other* scripts, so re-saving the same name
	// does not turn `notes.sau` into `notes-2.sau`.
	const taken = new Set(scripts.filter((s) => s.id !== id).map((s) => s.name.toLowerCase()));
	if (taken.has(next.toLowerCase())) {
		const stem = next.replace(/\.sau$/i, '');
		let n = 2;
		while (taken.has(`${stem}-${n}.sau`.toLowerCase())) n += 1;
		script.name = `${stem}-${n}.sau`;
	} else {
		script.name = next;
	}
	script.updatedAt = Date.now();
	writeRaw(scripts);
	return script;
}

export function deleteScript(id: string) {
	writeRaw(readRaw().filter((s) => s.id !== id));
	if (lastOpened() === `user:${id}`) rememberLast(null);
}

/*
 * Which entry the picker had selected last, as its option value — so coming
 * back to `/play/` reopens the script you were writing rather than dropping
 * you on "Hello, world" again. A shared link in the URL still wins over it.
 */
const LAST_KEY = 'saule.playground.last.v1';

export function lastOpened(): string | null {
	try {
		return window.localStorage.getItem(LAST_KEY);
	} catch {
		return null;
	}
}

export function rememberLast(value: string | null) {
	try {
		if (value === null) window.localStorage.removeItem(LAST_KEY);
		else window.localStorage.setItem(LAST_KEY, value);
	} catch {
		// Nothing to remember with. The session still works.
	}
}
