/**
 * The current `stable` and `latest` releases, read at build time for the
 * header's release badge (see `components/ReleaseVersions.astro`).
 *
 * **Not through the GitHub API.** The unauthenticated API allows 60 requests
 * an hour per IP, shared by everything on the network: a build used to spend
 * that on its own, and then the browsers on the same network — the person
 * checking the deploy — were refused for the rest of the hour. These are the
 * two sources the installer reads (`www/public/install.sh`), neither of which
 * is rate-limited:
 *
 * - `/releases/latest` redirects to the newest release that is not a
 *   pre-release — exactly the `stable` channel;
 * - `releases.atom` lists every published release, newest first, pre-releases
 *   included. Its first entry is the `latest` channel, shown only when it is
 *   not the stable one.
 *
 * **Fetched once per build.** The badge renders on every page; the promise is
 * held at module level so the sources are asked once, not once per page.
 */
import { repo } from '../../site.config.mjs';

export interface ReleaseEntry {
	version: string;
	url: string;
}

export interface ReleaseChannels {
	stable?: ReleaseEntry;
	latest?: ReleaseEntry;
}

let pending: Promise<ReleaseChannels> | undefined;

export function buildTimeChannels(): Promise<ReleaseChannels> {
	pending ??= load();
	return pending;
}

const entry = (tag: string): ReleaseEntry => ({
	version: tag.replace(/^v/, ''),
	url: `${repo}/releases/tag/${tag}`,
});

async function load(): Promise<ReleaseChannels> {
	const out: ReleaseChannels = {};
	let stableTag: string | undefined;
	try {
		const res = await fetch(`${repo}/releases/latest`, {
			redirect: 'manual',
			signal: AbortSignal.timeout(5000),
		});
		stableTag = res.headers.get('location')?.match(/\/releases\/tag\/([^/?#]+)/)?.[1];
		if (stableTag) out.stable = entry(stableTag);
	} catch {
		// Offline build: the browser fills the badge in instead.
	}
	try {
		const res = await fetch(`${repo}/releases.atom`, { signal: AbortSignal.timeout(5000) });
		if (res.ok) {
			const newest = (await res.text()).match(/\/releases\/tag\/([^"<\s]+)/)?.[1];
			if (newest && newest !== stableTag) out.latest = entry(newest);
		}
	} catch {
		// As above.
	}
	return out;
}
