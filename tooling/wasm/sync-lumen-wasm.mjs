import { copyFileSync, existsSync, mkdirSync } from 'node:fs'
import { resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const targetRoot = resolve(rootDir, 'crates/lumen-wasm/target/wasm32-unknown-emscripten')
const buildDirs = ['release', 'debug']
const artifacts = ['lumen_wasm.js', 'lumen_wasm.wasm']

const sourceDir = buildDirs
	.map((dir) => resolve(targetRoot, dir))
	.find((dir) => artifacts.every((file) => existsSync(resolve(dir, file))))

if (!sourceDir) {
	console.warn(
		'[lumen-wasm] Missing wasm artifacts. Run `pnpm wasm:build` or `cargo build -p lumen-wasm --target wasm32-unknown-emscripten --release` first.',
	)
	process.exit(0)
}

const outputDir = resolve(rootDir, 'apps/editor/public/lumen-wasm')
mkdirSync(outputDir, { recursive: true })

for (const file of artifacts) {
	copyFileSync(resolve(sourceDir, file), resolve(outputDir, file))
}

console.log(`[lumen-wasm] Synced artifacts from ${sourceDir} to ${outputDir}`)
