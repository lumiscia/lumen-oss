#[global_allocator]
static ALLOCATOR: lumen_bench::bench::composition::frame_graph::CountingAlloc =
    lumen_bench::bench::composition::frame_graph::CountingAlloc;

fn main() -> anyhow::Result<()> {
    lumen_bench::bench::composition::frame_graph::run()
}
