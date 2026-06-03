use lumen_bench::bench::Bench;
use lumen_bench::bench::text::TextBench;

fn main() -> anyhow::Result<()> {
    TextBench::run()
}
