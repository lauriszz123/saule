import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

/**
 * The Saule TextMate grammar, loaded straight out of `grammar/`.
 *
 * There is deliberately no copy of the grammar under `www/` — a duplicate
 * would drift the moment a keyword is added, and the website would highlight
 * Saule differently than the editors do. Reading the single source of truth
 * means `grammar/saule.tmLanguage.json` is the only file that ever needs to
 * change; the VS Code extension takes its copy from there too, with
 * `npm run sync:grammar` in the `saule-vscode` repository.
 */
const grammarPath = fileURLToPath(
	new URL('../../../grammar/saule.tmLanguage.json', import.meta.url)
);

let raw;
try {
	raw = JSON.parse(readFileSync(grammarPath, 'utf8'));
} catch (cause) {
	throw new Error(
		`Could not read the Saule TextMate grammar at ${grammarPath}. ` +
			'The website builds from inside the language repo and reads the ' +
			'grammar from `grammar/` directly.',
		{ cause }
	);
}

export default {
	...raw,
	// Shiki keys languages by `name`, and the grammar spells it
	// "Saule" — which would force every code fence to be ```Saule.
	name: 'saule',
	aliases: ['sau'],
};
