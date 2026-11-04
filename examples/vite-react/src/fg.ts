// Spike page: times a composition's frames in the preview, on the main thread.
import { createLumenBindings } from "@lumiscia/lumen-bindings/bundler";
import { createLumenPreviewRuntime, resolveLumenPreviewBindings } from "@lumiscia/lumen-preview";

const FONTS: [string, string][] = [
  ["Geist", "Geist[wght].ttf"],
  ["Geist Mono", "GeistMono[wght].ttf"],
  ["Instrument Serif", "InstrumentSerif-Regular.ttf"],
];

const parameters = new URLSearchParams(location.search);
const passes = Number(parameters.get("passes") ?? 3);
const frames = Number(parameters.get("frames") ?? 600);

async function run() {
  const bindings = await resolveLumenPreviewBindings(createLumenBindings());
  const { LumenPreviewController } = createLumenPreviewRuntime(bindings);
  const controller = new LumenPreviewController();
  for (const [family, file] of FONTS) {
    const bytes = await (await fetch(`/fg/fonts/${encodeURIComponent(file)}`)).arrayBuffer();
    controller.registerFontFamily(family, bytes);
  }
  const composition = await (await fetch(`/fg/${parameters.get("composition")}`)).text();
  const started = performance.now();
  controller.loadComposition(composition);
  const canvas = document.getElementById("canvas") as HTMLCanvasElement;
  const count = Math.min(frames, controller.durationFrames());
  const samples: number[][] = [];
  const paced: number[] = [];
  for (let pass = 0; pass < passes; pass += 1) {
    const times: number[] = [];
    for (let frame = 0; frame < count; frame += 1) {
      controller.setFrame(frame);
      const before = performance.now();
      await controller.renderNowAsync(canvas);
      times.push(performance.now() - before);
      // One frame per display refresh, as playback renders.
      const previous = performance.now();
      await new Promise(requestAnimationFrame);
      paced.push(performance.now() - previous);
    }
    samples.push(times);
  }
  return { samples, paced, setupMs: samples[0]?.[0] ?? 0, wallMs: performance.now() - started };
}

(window as unknown as { fgResult: Promise<unknown> }).fgResult = run();
