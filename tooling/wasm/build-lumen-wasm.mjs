import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const targetDir = resolve(rootDir, 'target/wasm32-unknown-emscripten/release')
const staticLib = resolve(targetDir, 'liblumen_wasm.a')
const outputJs = resolve(targetDir, 'lumen_wasm.js')

if (!existsSync(staticLib)) {
	console.error(
		'[lumen-wasm] Missing static library. Run `cargo build -p lumen-wasm --target wasm32-unknown-emscripten --release` first.',
	)
	process.exit(1)
}

const exportedFunctions = [
	'_malloc',
	'_free',
	'_lumen_renderer_create',
	'_lumen_renderer_destroy',
	'_lumen_renderer_width',
	'_lumen_renderer_height',
	'_lumen_renderer_render_frame',
	'_lumen_renderer_last_frame_len',
	'_lumen_renderer_frame_requirements',
	'_lumen_renderer_frame_requirements_len',
	'_lumen_renderer_last_error_ptr',
	'_lumen_renderer_last_error_len',
	'_lumen_media_create',
	'_lumen_media_destroy',
	'_lumen_media_clear',
	'_lumen_media_set_image',
	'_lumen_media_set_video_frame',
]

const exportArg = `EXPORTED_FUNCTIONS=[${exportedFunctions.map((fn) => `'${fn}'`).join(',')}]`

execFileSync(
	'emcc',
	[
		staticLib,
		'-o',
		outputJs,
		'--no-entry',
		'-s',
		exportArg,
		'-s',
		"EXPORTED_RUNTIME_METHODS=['HEAPU8']",
		'-s',
		'MODULARIZE=1',
		'-s',
		'EXPORT_ES6=1',
		'-s',
		'ENVIRONMENT=web,worker',
		'-s',
		'ALLOW_MEMORY_GROWTH=1',
		'-s',
		'INITIAL_MEMORY=268435456',
		'-s',
		'WASM_BIGINT=1',
		'-s',
		'DISABLE_EXCEPTION_CATCHING=0',
		'-s',
		'ABORTING_MALLOC=0',
	],
	{ stdio: 'inherit' },
)
