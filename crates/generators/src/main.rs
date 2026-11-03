use std::{env, path::Path};

use anyhow::{Result, anyhow};

fn main() -> Result<()> {
    let out_dir = env::args()
        .nth(1)
        .ok_or_else(|| anyhow!("usage: lumen-generators <out-dir>"))?;
    lumen_generators::generate(Path::new(&out_dir))
}
