use std::env;

use anyhow::Result;
use lumen_generators::{generate, parse_args_from};

fn main() -> Result<()> {
    let args = parse_args_from(env::args())?;
    generate(&args)
}
