const CHANNELS = 2;
const SAMPLE_RATE = 48_000;
const CLIP_EDGE_FADE_SAMPLES = 384;

let timeline = null;
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
    case "request-chunk":
      postChunk(message);
      break;
  }
};

function postChunk({ chunkFrames, generation, startSample }) {
  if (!timeline) {
    return;
  }
  const samples = mixTimelineChunk(timeline, sources, startSample, chunkFrames);
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
 * Mixes interleaved stereo for the export mixer's rules, looping over the timeline's duration.
 * `crates/engine/src/audio.rs` writes the fixtures `audio-worker.test.ts` checks this against.
 */
export function mixTimelineChunk(nextTimeline, nextSources, startSample, frames) {
  const output = new Float32Array(frames * CHANNELS);
  const durationMs = (nextTimeline.durationFrames / Math.max(nextTimeline.fps, 1)) * 1_000;
  const loopSamples =
    Number.isFinite(durationMs) && durationMs > 0
      ? (durationMs * SAMPLE_RATE) / 1_000
      : Number.POSITIVE_INFINITY;

  let outputFrame = 0;
  while (outputFrame < frames) {
    const sample = startSample + outputFrame;
    const [loopStart, loopEnd] = loopBounds(sample, loopSamples);
    const timelineStart = sample - loopStart;
    const segmentFrames = Math.min(frames - outputFrame, loopEnd - sample);
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

// Loop k covers playback samples [floor(k * loopSamples), floor((k + 1) * loopSamples)), so a
// fractional loop length stays in phase with the millisecond clock across repetitions.
function loopBounds(sample, loopSamples) {
  if (!Number.isFinite(loopSamples)) {
    return [0, Number.POSITIVE_INFINITY];
  }
  let loop = Math.floor(sample / loopSamples);
  if (sample >= Math.floor((loop + 1) * loopSamples)) {
    loop += 1;
  }
  return [Math.floor(loop * loopSamples), Math.floor((loop + 1) * loopSamples)];
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
