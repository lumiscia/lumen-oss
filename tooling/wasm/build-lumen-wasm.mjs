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

if (!process.env.EMSDK) {
	console.warn(
		'[lumen-wasm] EMSDK is not set; ensure your emscripten toolchain is discoverable (asdf-managed emsdk is also supported).',
	)
}

const exportedFunctions = [
	'_malloc',
	'_free',
	'_lumen_wasm_version',
	'_lumen_wasm_last_status_ptr',
	'_lumen_wasm_last_status_len',
	'_lumen_wasm_load_project',
	'_lumen_wasm_unload_project',
	'_lumen_wasm_project_width',
	'_lumen_wasm_project_height',
	'_lumen_wasm_request_frame',
	'_lumen_wasm_request_frame_len',
	'_lumen_wasm_request_frame_requirements',
	'_lumen_wasm_request_frame_requirements_len',
	'_lumen_wasm_last_error_ptr',
	'_lumen_wasm_last_error_len',
	'_lumen_wasm_media_store_create',
	'_lumen_wasm_media_store_destroy',
	'_lumen_wasm_media_store_clear',
	'_lumen_wasm_media_store_clear_videos',
	'_lumen_wasm_media_store_has_image',
	'_lumen_wasm_media_store_set_image',
	'_lumen_wasm_media_store_set_video_frame',
	'_lumen_wasm_media_store_set_image_owned',
	'_lumen_wasm_media_store_set_video_frame_owned',
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
		"EXPORTED_RUNTIME_METHODS=['HEAPU8','GL']",
		'-s',
		'ERROR_ON_UNDEFINED_SYMBOLS=0',
		'-s',
		'MAX_WEBGL_VERSION=2',
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
