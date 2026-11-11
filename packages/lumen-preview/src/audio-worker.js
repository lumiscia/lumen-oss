const CHANNELS = 2;
const SAMPLE_RATE = 48_000;
const CLIP_EDGE_FADE_SAMPLES = 384;

let timeline = null;
let range = null;
const sources = new Map();

globalThis.onmessage = (event) => {
  const message = event.data;
  switch (message.type) {
    case "remove-source":
      sources.delete(message.id);
      break;
    case "set-source":
      sources.set(message.id, message.samples);
      break;
    case "set-timeline":
      timeline = message.timeline;
      break;
    case "set-range":
      range = message.range;
      break;
    case "request-chunk":
      postChunk(message);
      break;
  }
};

function postChunk({ chunkFrames, generation, startSample }) {
  if (!timeline) {
    return;
  }
  const samples = mixTimelineChunk(timeline, sources, startSample, chunkFrames, range);
  globalThis.postMessage(
    {
      type: "chunk",
      generation,
      startSample,
      frames: chunkFrames,
      sampleRate: SAMPLE_RATE,
      channelCount: CHANNELS,
      samples,
    },
    [samples.buffer],
  );
}

/**
 * Mixes interleaved stereo from the engine's audio plan. Playback sample `startSample` is a
 * position in the timeline that keeps counting past the end of `range`: a looping range repeats,
 * and one that doesn't is silent from its end. Without `range` the whole timeline loops.
 * `crates/engine/src/audio/mix.rs` plays plans the same way and writes the fixtures
 * `audio-worker.test.ts` checks this against.
 *
 * @param {{ startMs: number, endMs: number, loop: boolean } | null} [range] `endMs` is exclusive.
 */
export function mixTimelineChunk(nextTimeline, nextSources, startSample, frames, range = null) {
  const output = new Float32Array(frames * CHANNELS);
  const durationMs = (nextTimeline.durationFrames / Math.max(nextTimeline.fps, 1)) * 1_000;
  const startMs = range ? range.startMs : 0;
  const endMs = range ? range.endMs : durationMs;
  const loop = range ? range.loop : true;
  const rangeStart = (startMs * SAMPLE_RATE) / 1_000;
  const rangeSamples =
    Number.isFinite(endMs) && endMs > startMs
      ? ((endMs - startMs) * SAMPLE_RATE) / 1_000
      : Number.POSITIVE_INFINITY;
  const firstSample = Math.floor(rangeStart);
  const endSample = Math.floor(rangeStart + rangeSamples);

  let outputFrame = 0;
  while (outputFrame < frames) {
    const sample = startSample + outputFrame;
    let timelineStart;
    let segmentFrames;
    if (loop) {
      const [loopStart, loopEnd] = loopBounds(sample, rangeStart, rangeSamples);
      timelineStart = firstSample + sample - loopStart;
      segmentFrames = Math.min(frames - outputFrame, loopEnd - sample);
    } else if (sample >= endSample) {
      break;
    } else if (sample < firstSample) {
      outputFrame += Math.min(frames - outputFrame, firstSample - sample);
      continue;
    } else {
      timelineStart = sample;
      segmentFrames = Math.min(frames - outputFrame, endSample - sample);
    }
    mixPlanRange(
      nextTimeline,
      nextSources,
      output,
      outputFrame,
      timelineStart,
      timelineStart + segmentFrames,
    );
    outputFrame += segmentFrames;
  }

  for (let index = 0; index < output.length; index += 1) {
    const sample = output[index];
    output[index] = Number.isNaN(sample) ? 0 : Math.min(1, Math.max(-1, sample));
  }
  return output;
}

/**
 * Adds plan samples `[startSample, endSample)` to `output` from `outputStartFrame`. Segment sample
 * `i` plays its voice's clip at `position + step * i`, between `from` and `to`, at a gain moving
 * linearly across the segment, with the clip's edge ramps and equal-power fades, reading the
 * source between whole samples linearly.
 */
function mixPlanRange(plan, nextSources, output, outputStartFrame, startSample, endSample) {
  for (const segment of plan.segments) {
    const voice = plan.voices[segment.voice];
    const source = voice && nextSources.get(voice.sourceId);
    if (!source) {
      continue;
    }
    const first = Math.max(0, startSample - segment.start);
    const last = Math.min(segment.frames, endSample - segment.start);
    const sourceFrames = source.length / CHANNELS;
    for (let i = first; i < last; i += 1) {
      const position = segment.position + segment.step * i;
      if (!(position >= segment.from && position < segment.to)) {
        continue;
      }
      const along = i / segment.frames;
      const gain =
        (segment.gain[0] + (segment.gain[1] - segment.gain[0]) * along) *
        edgeFade(position - segment.from, segment.to - segment.from) *
        fade(position, voice.fadeIn) *
        fade(voice.duration - position, voice.fadeOut);
      const sourcePosition = voice.sourceStart + position;
      const whole = Math.floor(sourcePosition);
      const fraction = sourcePosition - whole;
      const outputFrame = outputStartFrame + segment.start + i - startSample;
      for (let channel = 0; channel < CHANNELS; channel += 1) {
        const at = sourceSample(source, sourceFrames, whole, channel);
        const value =
          fraction === 0
            ? at
            : at * (1 - fraction) +
              sourceSample(source, sourceFrames, whole + 1, channel) * fraction;
        output[outputFrame * CHANNELS + channel] += value * gain;
      }
    }
  }
}

function sourceSample(source, sourceFrames, frame, channel) {
  return frame >= 0 && frame < sourceFrames ? source[frame * CHANNELS + channel] : 0;
}

// Loop k covers playback samples [floor(start + k * length), floor(start + (k + 1) * length)), so a
// fractional loop length stays in phase with the millisecond clock across repetitions.
function loopBounds(sample, start, length) {
  if (!Number.isFinite(length)) {
    return [0, Number.POSITIVE_INFINITY];
  }
  let loop = Math.floor((sample - start) / length);
  if (sample >= Math.floor(start + (loop + 1) * length)) {
    loop += 1;
  }
  return [Math.floor(start + loop * length), Math.floor(start + (loop + 1) * length)];
}

function edgeFade(offset, length) {
  const fadeSamples = Math.min(CLIP_EDGE_FADE_SAMPLES, Math.floor(length / 2));
  if (fadeSamples <= 0) {
    return 1;
  }
  if (offset < fadeSamples) {
    return offset / fadeSamples;
  }
  const remaining = length - offset;
  return remaining < fadeSamples ? remaining / fadeSamples : 1;
}

function fade(distance, length) {
  return length > 0 && distance < length
    ? Math.sin((Math.max(0, distance) / length) * (Math.PI / 2))
    : 1;
}
