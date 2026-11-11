import { describe, expect, it, vi } from "vitest";

import type { PreviewClock } from "../src/clock.js";
import type { PlaybackRange } from "../src/playback-range.js";
import { PreviewRunner } from "../src/runner.js";

const FPS = 10;
const DURATION = 20;
const LOOP: PlaybackRange = { inFrame: 0, loop: true, outFrame: DURATION };

/** A fake clock and timer queue, where a render takes no time unless a test holds it. */
function createRunner() {
  const timers = new Map<number, { at: number; callback: () => void }>();
  const failures: unknown[] = [];
  const ended: number[] = [];
  const frames: { frame: number; renderMs: number | null; seekId: number }[] = [];
  const rendered: number[] = [];
  const held: (() => void)[] = [];
  let holdRenders = false;
  let nextFailure: Error | null = null;
  let now = 0;
  let nextTimer = 1;
  let playhead = 0;

  const runner = new PreviewRunner<number>(
    {
      prepareFrame: () =>
        holdRenders ? new Promise((resolve) => held.push(resolve)) : Promise.resolve(),
      renderFrame: async () => {
        if (nextFailure) {
          const failure = nextFailure;
          nextFailure = null;
          throw failure;
        }
        rendered.push(playhead);
      },
      setFrame: (frame) => {
        playhead = frame;
      },
    },
    {
      clearTimer: (timer) => timers.delete(timer),
      now: () => now,
      onEnded: (seekId) => ended.push(seekId),
      onFailure: (error) => failures.push(error),
      onFrame: (frame, seekId, renderMs) => frames.push({ frame, renderMs, seekId }),
      setTimer: (callback, delayMs) => {
        timers.set(nextTimer, { at: now + delayMs, callback });
        return nextTimer++;
      },
    },
  );

  // Runs everything due now: renders go through a zero-delay timer between each other.
  const settle = async () => {
    for (let turn = 0; turn < 50; turn += 1) {
      await Promise.resolve();
      for (const [id, timer] of timers) {
        if (timer.at <= now) {
          timers.delete(id);
          timer.callback();
        }
      }
    }
  };

  return {
    ended,
    failures,
    frames,
    held,
    rendered,
    runner,
    settle,
    get now() {
      return now;
    },
    clock(originMs: number, rate: number): PreviewClock {
      return { atEpochMs: now, originMs, rate };
    },
    failNextRender(error: Error) {
      nextFailure = error;
    },
    holdRenders(hold: boolean) {
      holdRenders = hold;
    },
    async advance(ms: number) {
      const end = now + ms;
      for (;;) {
        await settle();
        const due = [...timers].filter(([, timer]) => timer.at <= end);
        due.sort(([, left], [, right]) => left.at - right.at);
        const next = due[0];
        if (!next) {
          break;
        }
        timers.delete(next[0]);
        now = Math.max(now, next[1].at);
        next[1].callback();
      }
      now = end;
      await settle();
    },
  };
}

async function loaded() {
  const fake = createRunner();
  fake.runner.load(DURATION, FPS, fake.clock(0, 0), LOOP);
  await fake.settle();
  return fake;
}

describe("PreviewRunner", () => {
  it("renders each frame once while the clock runs, and nothing while it's parked", async () => {
    const fake = await loaded();
    expect(fake.rendered).toEqual([0]);

    await fake.advance(500);
    expect(fake.rendered).toEqual([0]);

    fake.runner.setClock(fake.clock(0, 1));
    await fake.advance(350);
    expect(fake.rendered).toEqual([0, 1, 2, 3]);
  });

  it("starts a timeline on its clock, parked within the timeline", async () => {
    const fake = createRunner();
    fake.runner.load(DURATION, FPS, fake.clock(700, 0), LOOP);
    await fake.settle();
    fake.runner.load(DURATION, FPS, fake.clock(5_000, 0), LOOP);
    await fake.settle();
    expect(fake.rendered).toEqual([7, 19]);
  });

  it("parks a pause past the end of the loaded timeline on its last frame", async () => {
    const fake = await loaded();
    fake.runner.setClock(fake.clock(5_000, 0));
    await fake.settle();
    expect(fake.rendered).toEqual([0, 19]);
  });

  it("keeps the clock it follows through a load whose clock is only jitter", async () => {
    const fake = await loaded();
    fake.runner.setClock(fake.clock(0, 1));
    await fake.advance(1_005);
    const before = fake.rendered.length;
    expect(fake.rendered.at(-1)).toBe(10);

    fake.runner.unload();
    fake.runner.load(DURATION, FPS, fake.clock(995, 1), LOOP);
    await fake.advance(50);
    expect(fake.rendered.slice(before)).toEqual([10]);
  });

  it("wraps at the end of the timeline", async () => {
    const fake = await loaded();
    fake.runner.seek(1, fake.clock(1_900, 1));
    await fake.advance(250);
    expect(fake.rendered.slice(-3)).toEqual([19, 0, 1]);
  });

  it("holds through a suspended clock and resumes from the same place", async () => {
    const fake = await loaded();
    fake.runner.setClock(fake.clock(0, 1));
    await fake.advance(250);
    expect(fake.rendered.at(-1)).toBe(2);

    fake.runner.setClock(fake.clock(250, 0));
    await fake.advance(5_000);
    expect(fake.rendered.at(-1)).toBe(2);

    fake.runner.setClock(fake.clock(250, 1));
    await fake.advance(100);
    expect(fake.rendered).toEqual([0, 1, 2, 3]);
  });

  it("ignores a correction that's only jitter, and follows one that's a real change", async () => {
    const fake = await loaded();
    fake.runner.setClock(fake.clock(0, 1));
    await fake.advance(199);
    expect(fake.rendered).toEqual([0, 1]);

    // Measured 20 ms ago and delivered late, so it reads just past the next frame.
    fake.runner.setClock({ atEpochMs: fake.now - 20, originMs: 205 - 20, rate: 1 });
    await fake.settle();
    expect(fake.rendered).toEqual([0, 1]);

    fake.runner.setClock(fake.clock(1_000, 1));
    await fake.settle();
    expect(fake.rendered.at(-1)).toBe(10);
  });

  it("treats a correction from the audio's wrapped position as no change", async () => {
    const fake = await loaded();
    fake.runner.seek(1, fake.clock(1_950, 1));
    await fake.advance(100);
    const before = fake.rendered.length;

    fake.runner.setClock(fake.clock(50, 1));
    await fake.settle();
    expect(fake.rendered).toHaveLength(before);
  });

  it("jumps to where the clock is when a slow render finishes", async () => {
    const fake = await loaded();
    fake.holdRenders(true);
    fake.runner.setClock(fake.clock(0, 1));
    await fake.advance(100);
    await fake.advance(500);
    fake.holdRenders(false);
    fake.held.splice(0).forEach((release) => release());
    await fake.settle();
    expect(fake.rendered.at(-1)).toBe(6);
    expect(fake.rendered).not.toContain(3);
  });

  it("drops a render a newer seek overtakes, and acknowledges only the newest", async () => {
    const fake = await loaded();
    fake.holdRenders(true);
    fake.runner.seek(1, fake.clock(500, 0));
    await fake.settle();
    fake.runner.seek(2, fake.clock(1_000, 0));
    fake.holdRenders(false);
    fake.held.splice(0).forEach((release) => release());
    await fake.settle();

    expect(fake.rendered.at(-1)).toBe(10);
    expect(fake.rendered).not.toContain(5);
    expect(fake.frames.at(-1)).toMatchObject({ frame: 10, seekId: 2 });
  });

  it("acknowledges a seek to the frame on screen without rendering", async () => {
    const fake = await loaded();
    fake.runner.seek(1, fake.clock(10, 0));
    await fake.settle();
    expect(fake.rendered).toEqual([0]);
    expect(fake.frames.at(-1)).toEqual({ frame: 0, renderMs: null, seekId: 1 });
  });

  it("stops on a failed render until playing or a seek asks again", async () => {
    const fake = await loaded();
    fake.failNextRender(new Error("device lost"));
    fake.runner.setClock(fake.clock(0, 1));
    await fake.advance(250);
    expect(fake.failures).toHaveLength(1);
    expect(fake.rendered).toEqual([0]);

    fake.runner.setClock(fake.clock(100, 0));
    await fake.advance(100);
    expect(fake.rendered).toEqual([0]);

    fake.runner.seek(1, fake.clock(100, 0));
    await fake.settle();
    expect(fake.rendered).toEqual([0, 1]);
  });

  it("drops frames and still answers pause and seek when renders can't keep up", async () => {
    const RENDER_MS = 40;
    const FAST_FPS = 60;
    // Without yielding, the loop never lets a timer fire. This ends it instead of hanging the test.
    const MAX_RENDERS = 25;
    const failures: unknown[] = [];
    const answered: { frame: number; seekId: number }[] = [];
    const rendered: number[] = [];
    let playhead = 0;
    const runner = new PreviewRunner<ReturnType<typeof setTimeout>>(
      {
        prepareFrame: () => Promise.resolve(),
        renderFrame: () => {
          if (rendered.length >= MAX_RENDERS) {
            throw new Error("the render loop never yielded");
          }
          const until = performance.now() + RENDER_MS;
          while (performance.now() < until);
          rendered.push(playhead);
          return Promise.resolve();
        },
        setFrame: (frame) => {
          playhead = frame;
        },
      },
      {
        clearTimer: (timer) => clearTimeout(timer),
        now: () => performance.now(),
        onEnded: () => {},
        onFailure: (error) => failures.push(error),
        onFrame: (frame, seekId) => answered.push({ frame, seekId }),
        setTimer: (callback, delayMs) => setTimeout(callback, delayMs),
      },
    );
    const clock = (originMs: number, rate: number): PreviewClock => ({
      atEpochMs: performance.now(),
      originMs,
      rate,
    });
    const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

    runner.load(10_000, FAST_FPS, clock(0, 0), { inFrame: 0, loop: true, outFrame: 10_000 });
    runner.setClock(clock(0, 1));
    await wait(300);
    expect(failures).toEqual([]);
    expect(rendered.at(-1)).toBeGreaterThan(rendered.length);

    runner.seek(1, clock(5_000, 0));
    await vi.waitFor(() => expect(answered.at(-1)).toEqual({ frame: 300, seekId: 1 }));

    const renders = rendered.length;
    await wait(RENDER_MS * 3);
    expect(rendered).toHaveLength(renders);
    expect(failures).toEqual([]);
  });

  it("drops what a replaced timeline was doing", async () => {
    const fake = await loaded();
    fake.holdRenders(true);
    fake.runner.seek(1, fake.clock(500, 0));
    await fake.settle();

    fake.runner.unload();
    fake.holdRenders(false);
    fake.runner.load(DURATION, FPS, fake.clock(0, 0), LOOP);
    fake.held.splice(0).forEach((release) => release());
    await fake.settle();

    expect(fake.rendered).toEqual([0, 0]);
  });
});

describe("PreviewRunner ranges", () => {
  async function inRange(range: PlaybackRange, originMs = 0) {
    const fake = createRunner();
    fake.runner.load(DURATION, FPS, fake.clock(originMs, 0), range);
    await fake.settle();
    return fake;
  }

  it("shows each frame of a looping range once per pass, with none skipped or repeated at the wrap", async () => {
    const fake = await inRange({ inFrame: 5, loop: true, outFrame: 10 }, 500);
    fake.runner.setClock(fake.clock(500, 1));
    await fake.advance(1_250);
    expect(fake.rendered).toEqual([5, 6, 7, 8, 9, 5, 6, 7, 8, 9, 5, 6, 7]);
    expect(fake.ended).toEqual([]);
  });

  it("keeps the wrap seamless at a faster rate", async () => {
    const fake = await inRange({ inFrame: 5, loop: true, outFrame: 10 }, 500);
    fake.runner.setClock(fake.clock(500, 2));
    await fake.advance(600);
    expect(fake.rendered).toEqual([5, 6, 7, 8, 9, 5, 6, 7, 8, 9, 5, 6, 7]);
  });

  it("holds the last frame of a range that doesn't loop, and says so once", async () => {
    const fake = await inRange({ inFrame: 5, loop: false, outFrame: 10 }, 500);
    fake.runner.setClock(fake.clock(500, 1));
    await fake.advance(450);
    expect(fake.ended).toEqual([]);

    await fake.advance(60);
    expect(fake.rendered).toEqual([5, 6, 7, 8, 9]);
    expect(fake.ended).toEqual([0]);

    await fake.advance(2_000);
    expect(fake.rendered).toEqual([5, 6, 7, 8, 9]);
    expect(fake.ended).toEqual([0]);
  });

  it("ends a playback that started past the end of its range on the last frame", async () => {
    const fake = await inRange({ inFrame: 0, loop: false, outFrame: 10 });
    fake.runner.seek(1, fake.clock(1_500, 1));
    await fake.advance(100);
    expect(fake.rendered).toEqual([0, 9]);
    expect(fake.ended).toEqual([1]);
  });

  it("doesn't end while parked at the end of a range", async () => {
    const fake = await inRange({ inFrame: 0, loop: false, outFrame: 10 });
    fake.runner.seek(1, fake.clock(900, 0));
    await fake.advance(2_000);
    expect(fake.rendered).toEqual([0, 9]);
    expect(fake.ended).toEqual([]);
  });

  it("follows a range that changes while playing", async () => {
    const fake = await inRange({ inFrame: 0, loop: true, outFrame: 20 });
    fake.runner.setClock(fake.clock(0, 1));
    await fake.advance(250);
    expect(fake.rendered).toEqual([0, 1, 2]);

    fake.runner.setRange({ inFrame: 10, loop: true, outFrame: 14 });
    await fake.advance(500);
    expect(fake.rendered.slice(3).every((frame) => frame >= 10 && frame < 14)).toBe(true);
    expect(fake.rendered.at(-1)).toBeGreaterThanOrEqual(10);
  });

  it("parks a pause before the range on its first frame", async () => {
    const fake = await inRange({ inFrame: 5, loop: false, outFrame: 10 });
    expect(fake.rendered).toEqual([5]);
  });
});
