use lumen_bench::bench::Bench;
use lumen_bench::bench::json_parse::JsonParseBench;

fn main() -> anyhow::Result<()> {
    JsonParseBench::run()
}
