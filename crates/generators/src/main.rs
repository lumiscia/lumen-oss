use std::env;

use anyhow::Result;
use lumen_generators::{Command, generate, parse_args_from, print_usage};

fn main() -> Result<()> {
    match parse_args_from(env::args())? {
        Command::Generate(args) => generate(&args),
        Command::Help => {
            print_usage();
            Ok(())
        }
    }
}
