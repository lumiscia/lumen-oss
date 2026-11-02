const DEFAULT_CHANNEL_COUNT = 2;
const DEFAULT_SAMPLE_RATE = 48_000;

export class LumenAudioWorkletProcessor extends globalThis.AudioWorkletProcessor {
  constructor() {
    super();
    this.channelCount = DEFAULT_CHANNEL_COUNT;
    this.currentSample = 0;
    this.generation = 0;
    this.chunks = new Map();
    this.playing = false;
    this.stalled = true;
    this.sampleRate = DEFAULT_SAMPLE_RATE;
    this.reportCountdown = 0;
    this.port.onmessage = (event) => this.handleMessage(event.data);
  }

  handleMessage(message) {
    try {
      switch (message.type) {
        case "chunk":
          this.storeChunk(message);
          break;
        case "play":
          this.generation = message.generation;
          this.currentSample = Number(message.startSample);
          this.chunks.clear();
          this.playing = true;
          this.stalled = true;
          this.reportPosition(0);
          break;
        case "pause":
          this.playing = false;
          break;
      }
    } catch (error) {
      this.port.postMessage({ type: "error", message: String(error) });
    }
  }

  storeChunk(message) {
    if (message.generation !== this.generation) {
      return;
    }
    this.channelCount = message.channelCount || DEFAULT_CHANNEL_COUNT;
    this.sampleRate = message.sampleRate || DEFAULT_SAMPLE_RATE;
    this.chunks.set(Number(message.startSample), {
      frames: message.frames,
      samples: message.samples,
      startSample: Number(message.startSample),
    });
  }

  process(_inputs, outputs) {
    const output = outputs[0];
    const left = output?.[0];
    if (!left) {
      return true;
    }

    for (const channel of output) {
      channel.fill(0);
    }
    if (!this.playing) {
      return true;
    }

    // The position only covers samples that were heard, so it waits out an underrun.
    const frames = left.length;
    let written = 0;
    for (; written < frames; written += 1) {
      const sample = this.currentSample + written;
      const chunk = this.findChunk(sample);
      if (!chunk) {
        break;
      }
      const chunkFrame = sample - chunk.startSample;
      const base = chunkFrame * this.channelCount;
      for (let channel = 0; channel < output.length; channel += 1) {
        const sourceChannel = Math.min(channel, this.channelCount - 1);
        output[channel][written] = chunk.samples[base + sourceChannel] || 0;
      }
    }

    this.currentSample += written;
    this.pruneChunks();
    this.reportCountdown -= frames;
    const stalled = written < frames;
    if (stalled !== this.stalled || this.reportCountdown <= 0) {
      this.stalled = stalled;
      this.reportPosition(frames);
    }
    return true;
  }

  findChunk(sample) {
    for (const chunk of this.chunks.values()) {
      if (sample >= chunk.startSample && sample < chunk.startSample + chunk.frames) {
        return chunk;
      }
    }
    return null;
  }

  pruneChunks() {
    const keepAfter = this.currentSample - this.sampleRate;
    for (const [startSample, chunk] of this.chunks) {
      if (chunk.startSample + chunk.frames < keepAfter) {
        this.chunks.delete(startSample);
      }
    }
  }

  reportPosition(framesRendered) {
    this.reportCountdown = Math.max(128, Math.floor(this.sampleRate / 20));
    this.port.postMessage({
      contextTime: globalThis.currentTime + framesRendered / this.sampleRate,
      currentSample: this.currentSample,
      generation: this.generation,
      stalled: this.stalled,
      type: "position",
    });
  }
}

globalThis.registerProcessor("lumen-audio-worklet", LumenAudioWorkletProcessor);
