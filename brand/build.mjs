#!/usr/bin/env node
// Rasterise the brand SVGs into the PNGs that cannot be SVG.
//
//   node brand/build.mjs
//
// Most places take the SVG directly — the website, the IntelliJ plugin, any
// README on GitHub. Only the VS Code Marketplace insists on a raster icon, and
// a few places (social cards, issue templates) are easier with one, so those
// are generated here rather than committed by hand and left to drift.
//
// sharp comes from the website's dependencies rather than a second install:
// this runs rarely, and a devDependency at the repo root just to redraw a logo
// is not worth the lockfile.

import { mkdir } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..');

let sharp;
try {
	sharp = createRequire(join(root, 'www', 'package.json'))('sharp');
} catch {
	console.error('sharp not found. Run `npm install` in www/ first.');
	process.exit(1);
}

// [source, output, size]. Everything is square and everything is transparent.
const TARGETS = [
	['saule-logo.svg', 'png/saule-logo-1024.png', 1024],
	['saule-logo.svg', 'png/saule-logo-512.png', 512],
	['saule-logo.svg', 'png/saule-logo-256.png', 256],
	['saule-logo.svg', 'png/saule-logo-128.png', 128],
	['saule-mark.svg', 'png/saule-mark-512.png', 512],
	['saule-mark.svg', 'png/saule-mark-256.png', 256],
	['saule-mark.svg', 'png/saule-mark-128.png', 128],
	['saule-mark-16.svg', 'png/saule-mark-32.png', 32],
	['saule-mark-16.svg', 'png/saule-mark-16.png', 16],
];

await mkdir(join(here, 'png'), { recursive: true });

for (const [src, out, size] of TARGETS) {
	// A high density renders the vector at full resolution before the resize,
	// so the gradients and the crescent's thin edge stay smooth at 16px.
	await sharp(join(here, src), { density: 900 })
		.resize(size, size, { fit: 'contain', background: { r: 0, g: 0, b: 0, alpha: 0 } })
		.png({ compressionLevel: 9 })
		.toFile(join(here, out));

	// A logo that ships with a baked-in background is no use on a dark README,
	// a light marketplace card, or an editor tab. Assert rather than trust.
	const { data, info } = await sharp(join(here, out)).raw().toBuffer({ resolveWithObject: true });
	const alpha = (x, y) => data[(y * info.width + x) * info.channels + 3];
	const corners = [
		alpha(0, 0),
		alpha(info.width - 1, 0),
		alpha(0, info.height - 1),
		alpha(info.width - 1, info.height - 1),
	];
	if (corners.some((a) => a !== 0)) {
		throw new Error(`${out} is not transparent at the corners: ${corners.join(', ')}`);
	}
	console.log(`  ${out}  ${size}x${size}`);
}

console.log(`\n${TARGETS.length} files, all transparent.`);
