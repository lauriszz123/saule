// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import sauleGrammar from './src/lib/saule-grammar.mjs';
// GitHub Pages serves this repo at https://lauriszz123.github.io/saule/, so
// every generated URL needs the `/saule` prefix. Both this config and the
// `sync-docs` script read it from one place — see site.config.mjs for what
// changes if a custom domain is added later.
import { site, base, repo } from './site.config.mjs';

export default defineConfig({
	site,
	base,
	trailingSlash: 'always',
	integrations: [
		starlight({
			title: 'Saule',
			description:
				'A statically typed, class-oriented scripting language with Lua’s simplicity and a real type system.',
			tagline: 'Lua’s simplicity. A real type system.',
			logo: {
				src: './src/assets/logo.svg',
				replacesTitle: false,
			},
			favicon: '/favicon.svg',
			social: [{ icon: 'github', label: 'GitHub', href: repo }],
			editLink: {
				baseUrl: `${repo}/edit/main/www/`,
			},
			customCss: ['./src/styles/theme.css'],
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
					frames: {
						shadowColor: 'transparent',
					},
				},
			},
			sidebar: [
				{
					label: 'Getting Started',
					items: [
						{ label: 'Introduction', slug: 'guides/introduction' },
						{ label: 'Installation', slug: 'guides/installation' },
						{ label: 'Your First Program', slug: 'guides/first-program' },
					],
				},
				{
					label: 'Language Guide',
					items: [{ autogenerate: { directory: 'language' } }],
				},
				{
					label: 'Standard Library',
					items: [{ autogenerate: { directory: 'stdlib' } }],
				},
				{
					label: 'Reference',
					items: [
						{ label: 'Quick Reference', slug: 'reference/quick-reference' },
						{ label: 'CLI', slug: 'reference/cli' },
						{ label: 'Editor Support', slug: 'reference/editors' },
					],
				},
				{
					label: 'Examples',
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
