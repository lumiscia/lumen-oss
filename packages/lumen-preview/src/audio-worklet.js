const DEFAULT_CHANNEL_COUNT = 2;
const DEFAULT_SAMPLE_RATE = 48_000;

class LumenAudioWorkletProcessor extends globalThis.AudioWorkletProcessor {
  constructor() {
    super();
    this.channelCount = DEFAULT_CHANNEL_COUNT;
    this.currentSample = 0;
    this.generation = 0;
    this.chunks = new Map();
    this.playing = false;
    this.sampleRate = DEFAULT_SAMPLE_RATE;
    this.needChunksCountdown = 0;
    this.port.onmessage = (event) => this.handleMessage(event.data);
    this.port.postMessage({ type: "ready" });
  }

  handleMessage(message) {
    try {
      switch (message.type) {
        case "reset":
          this.reset(message.generation, message.startSample ?? 0);
          break;
        case "chunk":
          this.storeChunk(message);
          break;
        case "play":
          this.generation = message.generation;
          this.currentSample = Number(message.startSample);
          this.playing = true;
          this.requestChunks();
          break;
        case "pause":
          this.playing = false;
          break;
        case "seek":
          this.generation = message.generation;
          this.currentSample = Number(message.startSample);
          this.chunks.clear();
          this.requestChunks();
          break;
      }
    } catch (error) {
      this.port.postMessage({ type: "error", message: String(error) });
    }
  }

  reset(generation, startSample) {
    this.generation = generation;
    this.currentSample = Number(startSample);
    this.chunks.clear();
    this.needChunksCountdown = 0;
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

    const frames = left.length;
    for (let frame = 0; frame < frames; frame += 1) {
      const sample = this.currentSample + frame;
      const chunk = this.findChunk(sample);
      if (!chunk) {
        continue;
      }
      const chunkFrame = sample - chunk.startSample;
      const base = chunkFrame * this.channelCount;
      for (let channel = 0; channel < output.length; channel += 1) {
        const sourceChannel = Math.min(channel, this.channelCount - 1);
        output[channel][frame] = chunk.samples[base + sourceChannel] || 0;
      }
    }

    this.currentSample += frames;
    this.pruneChunks();
    this.needChunksCountdown -= frames;
    if (this.needChunksCountdown <= 0) {
      this.requestChunks();
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

  requestChunks() {
    this.needChunksCountdown = Math.max(128, Math.floor(this.sampleRate / 20));
    this.port.postMessage({
      currentSample: this.currentSample,
      generation: this.generation,
      type: "need-chunks",
    });
  }
}

globalThis.registerProcessor("lumen-audio-worklet", LumenAudioWorkletProcessor);
