# @lumiscia/lumen-cli-linux-x64

The `lumen` binary for Linux on x64. Install [`@lumiscia/lumen-cli`](https://www.npmjs.com/package/@lumiscia/lumen-cli), which picks the package for your system.

## License

Lumen is Apache-2.0. This binary also contains FFmpeg built with its GPL parts and with x264 and x265, so the binary as a whole is distributed under GPL-3.0-or-later, the text in `LICENSE`. `licenses/` holds the license of each part: Lumen, each library linked in, the Rust crates in `rust-crates.txt` and the built-in Roboto font.

The source of version `x.y.z` is the [`vx.y.z` tag of lumen-oss](https://github.com/lumiscia/lumen-oss/tags). Its [`crates/cli/release`](https://github.com/lumiscia/lumen-oss/tree/main/crates/cli/release) pins the libraries and has the script that builds this binary.
