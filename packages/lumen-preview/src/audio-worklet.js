const DEFAULT_CHANNEL_COUNT = 2;
const DEFAULT_SAMPLE_RATE = 48_000;
const RECOVERY_RAMP_FRAMES = 240;

export class LumenAudioWorkletProcessor extends globalThis.AudioWorkletProcessor {
  constructor() {
    super();
    this.channelCount = DEFAULT_CHANNEL_COUNT;
    this.currentSample = 0;
    this.generation = 0;
    this.chunks = new Map();
    this.heardAudio = false;
    this.holdFrames = 0;
    this.playing = false;
    this.rate = 1;
    this.rateRevision = 0;
    this.recoveryFrames = 0;
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
          this.heardAudio = false;
          this.holdFrames = Number(message.holdFrames);
          this.rate = Number(message.rate) || 1;
          this.rateRevision = Number(message.rateRevision) || 0;
          this.recoveryFrames = 0;
          this.playing = true;
          this.stalled = this.holdFrames > 0;
          this.reportPosition(0);
          break;
        case "pause":
          this.playing = false;
          break;
        case "rate":
          this.rate = Number(message.rate) || 1;
          this.rateRevision = Number(message.rateRevision) || 0;
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

    // The position covers samples that were heard, until `holdFrames` runs out waiting for the first
    // of them. From then on it moves through missing samples in silence, so anything arriving late
    // is either behind it and never plays, or joins at the right place. Audio returning from a gap
    // ramps up, but the wait for the first chunk within `holdFrames` isn't a gap. The position
    // advances `rate` samples per output frame, and the samples between are interpolated, so pitch
    // follows speed.
    const frames = left.length;
    const rate = this.rate;
    let written = 0;
    for (; written < frames; written += 1) {
      const position = this.currentSample + written * rate;
      const sample = Math.floor(position);
      const chunk = this.findChunk(sample);
      if (chunk) {
        const fraction = position - sample;
        const next = fraction > 0 ? this.findChunk(sample + 1) : null;
        const gain = 1 - this.recoveryFrames / RECOVERY_RAMP_FRAMES;
        for (let channel = 0; channel < output.length; channel += 1) {
          const sourceChannel = Math.min(channel, this.channelCount - 1);
          const current =
            chunk.samples[(sample - chunk.startSample) * this.channelCount + sourceChannel] || 0;
          const following = next
            ? next.samples[(sample + 1 - next.startSample) * this.channelCount + sourceChannel] || 0
            : current;
          output[channel][written] = (current + (following - current) * fraction) * gain;
        }
        this.heardAudio = true;
        this.recoveryFrames = Math.max(0, this.recoveryFrames - 1);
      } else {
        if (this.heardAudio || this.holdFrames === 0) {
          this.recoveryFrames = RECOVERY_RAMP_FRAMES;
        }
        if (this.holdFrames > 0) {
          break;
        }
      }
    }

    this.currentSample += written * rate;
    if (!this.heardAudio) {
      this.holdFrames = Math.max(this.holdFrames - frames, 0);
    }
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
      rate: this.rate,
      rateRevision: this.rateRevision,
      stalled: this.stalled,
      type: "position",
    });
  }
}

globalThis.registerProcessor("lumen-audio-worklet", LumenAudioWorkletProcessor);
