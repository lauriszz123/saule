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
 * Source of truth for the code. The site is still *hosted* on GitHub Pages —
 * that is what keeps the installer URL stable — but development, CI and
 * releases all live on GitLab, so every link out of the docs points there.
 */
export const repo = 'https://gitlab.com/lauriszz12313/saule';

/**
 * Where a "edit this page" link goes. GitLab puts `/-/` in front of the verb,
 * so this cannot be derived from `repo` with the same suffix GitHub uses.
 */
export const editBase = `${repo}/-/edit/main/www/`;

/** Join `base` with a site-absolute path, avoiding a doubled slash. */
export function withBase(path) {
	const clean = path.startsWith('/') ? path : `/${path}`;
	return `${base.replace(/\/$/, '')}${clean}`;
}
