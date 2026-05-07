import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import * as topLevelAwaitPlugin from "vite-plugin-top-level-await";
import * as wasmPlugin from "vite-plugin-wasm";

const wasm = wasmPlugin.default as unknown as () => Plugin;
const topLevelAwait = topLevelAwaitPlugin.default as unknown as () => Plugin;

export default defineConfig({
  build: {
    target: "esnext",
  },
  plugins: [wasm(), topLevelAwait(), react()],
});
