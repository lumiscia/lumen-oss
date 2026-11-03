---
"@lumiscia/lumen-bindings": minor
---

For Rust: audio is 48 kHz stereo by contract, so `AudioBuffer` loses its sample rate and takes channels of equal length (`from_channels` panics otherwise), `AudioMetadata` is gone, and `AudioResolver` only has `resolve_range`. `AudioMixer::mix_into`, `AudioBuffer::silent` and `AudioBuffer::channels_mut` are gone; use `mix_range`. Mixed audio is unchanged.
