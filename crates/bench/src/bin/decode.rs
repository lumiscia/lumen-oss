use lumen_bench::bench::Bench;
use lumen_bench::bench::decode::DecodeBench;

fn main() -> anyhow::Result<()> {
    DecodeBench::run()
}
