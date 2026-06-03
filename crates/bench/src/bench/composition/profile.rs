use lumen_engine::gpu::CompiledComposition;
use lumen_gpu::PassDesc;

pub fn print_plan_profile(compiled: Option<&CompiledComposition>) {
    let Some(compiled) = compiled else {
        return;
    };
    println!(
        "plan_profile textures={} buffers={} programs={} passes={} compiled_nodes={}",
        compiled.plan.textures().len(),
        compiled.plan.buffers().len(),
        compiled.plan.programs().len(),
        compiled.plan.passes().len(),
        compiled.compiled_nodes.len(),
    );
    for pass in compiled.plan.passes() {
        let (kind, label) = match &pass.desc {
            PassDesc::Render(desc) => ("render", desc.label.as_deref()),
            PassDesc::Compute(desc) => ("compute", desc.label.as_deref()),
            PassDesc::CopyTexture(desc) => ("copy", desc.label.as_deref()),
        };
        println!(
            "plan_pass id={} kind={} label={}",
            pass.id.0,
            kind,
            label.unwrap_or("-")
        );
    }
}
