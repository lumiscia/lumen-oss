const CHANNELS = 2;
const SAMPLE_RATE = 48_000;
const CLIP_EDGE_FADE_SAMPLES = 384;

let timeline = null;
const sources = new Map();

globalThis.onmessage = (event) => {
  const message = event.data;
  switch (message.type) {
    case "clear-sources":
      sources.clear();
      break;
    case "remove-source":
      sources.delete(message.id);
      break;
    case "set-source":
      sources.set(message.id, message.samples);
      break;
    case "set-timeline":
      timeline = message.timeline;
      break;
    case "request-chunks":
      queueChunks(message);
      break;
  }
};

function queueChunks(message) {
  if (!timeline || timeline.clips.length === 0) {
    return;
  }
  for (
    let startSample = message.startSample;
    startSample <= message.throughSample;
    startSample += message.chunkFrames
  ) {
    const samples = mixTimelineChunk(timeline, sources, startSample, message.chunkFrames);
    globalThis.postMessage(
      {
        type: "chunk",
        generation: message.generation,
        startSample,
        frames: message.chunkFrames,
        sampleRate: SAMPLE_RATE,
        channelCount: CHANNELS,
        samples,
      },
      [samples.buffer],
    );
  }
}

function mixTimelineChunk(nextTimeline, nextSources, startSample, frames) {
  const output = new Float32Array(frames * CHANNELS);
  const hasSolo = nextTimeline.tracks.some((track) => track.solo);
  const tracks = new Map(nextTimeline.tracks.map((track) => [track.id, track]));
  const endSample = startSample + frames;

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
    const fadeSamples = Math.min(
      CLIP_EDGE_FADE_SAMPLES,
      Math.max(0, Math.floor(clipDurationSamples / 4)),
    );
    const sourceStartSample = msToSample(clip.sourceStartMs);
    for (let sample = overlapStartSample; sample < overlapEndSample; sample += 1) {
      const clipOffset = sample - clipStartSample;
      const sourceFrame = sourceStartSample + clipOffset;
      if (sourceFrame < 0 || sourceFrame * CHANNELS >= source.length) {
        continue;
      }

      const gain = baseGain * edgeFadeGain(clipOffset, clipDurationSamples, fadeSamples);
      const outputFrame = sample - startSample;
      for (let channel = 0; channel < CHANNELS; channel += 1) {
        const outputIndex = outputFrame * CHANNELS + channel;
        const sourceIndex = sourceFrame * CHANNELS + channel;
        output[outputIndex] = clampSample(
          (output[outputIndex] ?? 0) + (source[sourceIndex] ?? 0) * gain,
        );
      }
    }
  }

  return output;
}

function msToSample(ms) {
  return Math.floor((Math.max(0, ms) * SAMPLE_RATE) / 1_000);
}

function edgeFadeGain(offset, duration, fadeSamples) {
  if (fadeSamples <= 0) {
    return 1;
  }
  const fadeIn = Math.min(1, offset / fadeSamples);
  const fadeOut = Math.min(1, (duration - offset) / fadeSamples);
  return Math.max(0, Math.min(fadeIn, fadeOut));
}

function clampSample(sample) {
  if (sample > 1) {
    return 1;
  }
  if (sample < -1) {
    return -1;
  }
  return sample;
}
