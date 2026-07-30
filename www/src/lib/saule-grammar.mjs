import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

/**
 * The Saule TextMate grammar, loaded straight out of the VS Code extension.
 *
 * There is deliberately no copy of the grammar under `www/` — a duplicate
 * would drift the moment a keyword is added, and the website would highlight
 * Saule differently than the editor does. Reading the single source of truth
 * means `editors/vscode/syntaxes/saule.tmLanguage.json` is the only file that
 * ever needs to change.
 */
const grammarPath = fileURLToPath(
	new URL('../../../editors/vscode/syntaxes/saule.tmLanguage.json', import.meta.url)
);

let raw;
try {
	raw = JSON.parse(readFileSync(grammarPath, 'utf8'));
} catch (cause) {
	throw new Error(
		`Could not read the Saule TextMate grammar at ${grammarPath}. ` +
			'The website builds from inside the language repo and reads the ' +
			'VS Code extension’s grammar directly.',
		{ cause }
	);
}

export default {
	...raw,
	// Shiki keys languages by `name`, and the VS Code grammar spells it
	// "Saule" — which would force every code fence to be ```Saule.
	name: 'saule',
	aliases: ['sau'],
};
