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
 * Mixes interleaved stereo for the export mixer's rules. Playback sample `startSample` is a
 * position in the timeline that keeps counting past the end of `range`: a looping range repeats,
 * and one that doesn't is silent from its end. Without `range` the whole timeline loops.
 * `crates/engine/src/audio.rs` writes the fixtures `audio-worker.test.ts` checks this against.
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
    mixTimelineRange(
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
    output[index] = Math.min(1, Math.max(-1, output[index]));
  }
  return output;
}

function mixTimelineRange(
  nextTimeline,
  nextSources,
  output,
  outputStartFrame,
  startSample,
  endSample,
) {
  const hasSolo = nextTimeline.tracks.some((track) => track.solo);
  const tracks = new Map(nextTimeline.tracks.map((track) => [track.id, track]));

  for (const clip of nextTimeline.clips) {
    const track = tracks.get(clip.trackId);
    if (
      !track ||
      track.muted ||
      (hasSolo && !track.solo) ||
      track.volume <= 0 ||
      clip.volume <= 0
    ) {
      continue;
    }

    const source = nextSources.get(clip.sourceId);
    if (!source) {
      continue;
    }

    const clipStartSample = msToSample(clip.startMs);
    const clipDurationSamples = Math.max(1, msToSample(clip.durationMs));
    const clipEndSample = clipStartSample + clipDurationSamples;
    const overlapStartSample = Math.max(startSample, clipStartSample);
    const overlapEndSample = Math.min(endSample, clipEndSample);
    if (overlapEndSample <= overlapStartSample) {
      continue;
    }

    const baseGain = track.volume * clip.volume;
    const sourceStartSample = msToSample(clip.sourceStartMs);
    for (let sample = overlapStartSample; sample < overlapEndSample; sample += 1) {
      const clipOffset = sample - clipStartSample;
      const sourceFrame = sourceStartSample + clipOffset;
      if (sourceFrame * CHANNELS >= source.length) {
        break;
      }

      const gain = baseGain * clipEnvelope(clipOffset, clipDurationSamples);
      const outputFrame = outputStartFrame + sample - startSample;
      for (let channel = 0; channel < CHANNELS; channel += 1) {
        output[outputFrame * CHANNELS + channel] += source[sourceFrame * CHANNELS + channel] * gain;
      }
    }
  }
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

function msToSample(ms) {
  return Math.floor((Math.max(0, ms) * SAMPLE_RATE) / 1_000);
}

function clipEnvelope(clipSample, clipDuration) {
  const fade = Math.min(CLIP_EDGE_FADE_SAMPLES, Math.floor(clipDuration / 2));
  if (fade === 0) {
    return 1;
  }
  if (clipSample < fade) {
    return clipSample / fade;
  }
  const remaining = clipDuration - clipSample;
  return remaining < fade ? remaining / fade : 1;
}
