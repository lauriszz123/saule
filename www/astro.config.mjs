// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import sauleGrammar from './src/lib/saule-grammar.mjs';
// GitHub Pages serves this site at https://lauriszz123.github.io/saule/, so
// every generated URL needs the `/saule` prefix. Both this config and the
// `sync-docs` script read it from one place — see site.config.mjs for what
// changes if a custom domain is added later.
import { site, base, repo, editBase } from './site.config.mjs';

export default defineConfig({
	site,
	base,
	trailingSlash: 'always',
	integrations: [
		starlight({
			title: 'Saule',
			description:
				'A statically typed, class-oriented scripting language with Lua’s simplicity and a real type system.',
			tagline: 'Lua’s simplicity. Now it argues back.',
			logo: {
				src: './src/assets/logo.svg',
				replacesTitle: false,
			},
			favicon: '/favicon.svg',
			social: [{ icon: 'github', label: 'GitHub', href: repo }],
			editLink: {
				baseUrl: editBase,
			},
			customCss: ['./src/styles/theme.css'],
			// Only the search dialog is overridden, and only to fill its empty
			// state — the component wraps Starlight's own rather than replacing
			// it, so the Pagefind wiring and the Ctrl+K shortcut stay theirs.
			components: {
				Search: './src/components/Search.astro',
				// Adds the header's Docs button ahead of the social icons.
				SocialIcons: './src/components/SocialIcons.astro',
				// Carries the one site-wide script: the sliding tab indicator,
				// which needs to measure the selected tab.
				Head: './src/components/Head.astro',
			},
			expressiveCode: {
				themes: ['github-dark-default', 'github-light'],
				// Reuse the VS Code extension's TextMate grammar verbatim, so
				// website highlighting and editor highlighting can never drift.
				shiki: {
					langs: [sauleGrammar],
				},
				styleOverrides: {
					borderRadius: '0.5rem',
					codeFontFamily: 'var(--saule-font-mono)',
					// GitHub's themes bring their own cool grey backgrounds
					// (#f6f8fa and #0d1117). Against this site's warm neutrals
					// that inverts the colour temperature at the edge of every
					// code block, so the code surface comes from the palette
					// instead and only the token colours stay GitHub's.
					//
					// Deliberately just the surfaces: the frame chrome — the
					// terminal title bar with its three dots, the editor tab —
					// is what makes a `sh` block read as a terminal window, and
					// it is left to Expressive Code.
					codeBackground: 'var(--saule-code-bg)',
					frames: {
						shadowColor: 'transparent',
						terminalBackground: 'var(--saule-code-bg)',
					},
				},
			},
			sidebar: [
				// The index first, so every page in the docs has a way back to
				// the top of them — the header's Docs button is the other one,
				// and it is not on narrow screens.
				{
					label: 'Documentation',
					link: '/docs/',
				},
				{
					label: 'Getting Started',
					collapsed: true,
					items: [
						{ label: 'Introduction', slug: 'guides/introduction' },
						{ label: 'Installation', slug: 'guides/installation' },
						{ label: 'Your First Program', slug: 'guides/first-program' },
					],
				},
				{
					label: 'Language Guide',
					collapsed: true,
					items: [{ autogenerate: { directory: 'language' } }],
				},
				// Restored: removing it left eleven pages reachable only from
				// the index or from search, which is one step too many for the
				// half of the documentation people look things up in most.
				{
					label: 'Standard Library',
					collapsed: true,
					items: [{ autogenerate: { directory: 'stdlib' } }],
				},
				{
					label: 'Reference',
					collapsed: true,
					items: [
						{ label: 'Quick Reference', slug: 'reference/quick-reference' },
						{ label: 'Grammar', slug: 'reference/grammar' },
						{ label: 'CLI', slug: 'reference/cli' },
						{ label: 'Editor Support', slug: 'reference/editors' },
					],
				},
				{
					label: 'Examples',
					collapsed: true,
					items: [{ autogenerate: { directory: 'examples' } }],
				},
				{
					label: 'Playground',
					link: '/play/',
					attrs: { 'data-saule-play': 'true' },
				},
			],
			lastUpdated: true,
			pagination: true,
		}),
	],
});
