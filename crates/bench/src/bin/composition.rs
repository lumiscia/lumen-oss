use lumen_bench::bench::Bench;
use lumen_bench::bench::composition::CompositionBench;

fn main() -> anyhow::Result<()> {
    CompositionBench::run()
}
