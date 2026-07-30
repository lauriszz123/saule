/**
 * Values shared between the Astro config and the `sync-docs` script.
 *
 * `base` in particular has to agree in both places: Astro prefixes every
 * route with it, and the sync script has to bake the same prefix into the
 * cross-page links it rewrites out of README.md's in-page anchors.
 */
export const site = 'https://lauriszz123.github.io';
export const base = '/saule';
export const repo = 'https://github.com/lauriszz123/saule';

/** Join `base` with a site-absolute path, avoiding a doubled slash. */
export function withBase(path) {
	const clean = path.startsWith('/') ? path : `/${path}`;
	return `${base.replace(/\/$/, '')}${clean}`;
}
