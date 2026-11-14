---
"@lumiscia/lumen-bindings": minor
---

`lumen_ffmpeg::Encoder` is an encoder name parsed once into its `EncoderFamily` and the codec its spelling says, and the rules that depend on which encoder it is read those instead of matching the name again. `EncoderCandidate::encoder` and `VideoEncoderConfig::encoder` are an `Encoder` (`name()` is the FFmpeg name), `encoder_info`, `check_bit_depth` and `check_alpha` take `&Encoder`, `VideoEncoderConfig::cpu_rgba` and `VideoEncoderChoice::single` still take a name, and `EncoderFamily::of` is `Encoder::parse(name).family()`. `EncoderFamily` has a `Vaapi` variant, so code matching it exhaustively needs the new arm.
