/**
 * Values shared between the Astro config and the `sync-docs` script.
 *
 * `base` in particular has to agree in both places: Astro prefixes every
 * route with it, and the sync script has to bake the same prefix into the
 * cross-page links it rewrites out of README.md's in-page anchors.
 */
export const site = 'https://lauriszz123.github.io';
export const base = '/saule';

/**
 * Source of truth for the code, and where every link out of the docs points.
 * The site is hosted on GitHub Pages from the same project, which is what
 * keeps the installer URL stable.
 *
 * CI and release builds run on CircleCI (see CIRCLECI.md) — that is
 * infrastructure, not something the docs link to.
 */
export const repo = 'https://github.com/lauriszz123/saule';

/** Where an "edit this page" link goes. Starlight appends the file's path. */
export const editBase = `${repo}/edit/main/www/`;

/** Join `base` with a site-absolute path, avoiding a doubled slash. */
export function withBase(path) {
	const clean = path.startsWith('/') ? path : `/${path}`;
	return `${base.replace(/\/$/, '')}${clean}`;
}
