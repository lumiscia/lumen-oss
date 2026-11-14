import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    projects: [
      {
        test: {
          name: "node",
          include: [
            "packages/lumen-{cli,preview,sdk,shared}/test/**/*.test.ts",
            "packages/lumen-react/test/**/*.test.tsx",
            "tooling/generate-types/test/**/*.test.ts",
          ],
        },
      },
      // The Svelte plugin and the browser export condition belong to this package alone.
      "packages/lumen-svelte",
    ],
  },
});
