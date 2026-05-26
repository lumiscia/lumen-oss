use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    if env::var("CARGO_FEATURE_CUDA").is_err() {
        return;
    }

    // Kernel sources are Linux-only; other targets skip nvcc but still build the crate.
    if !cfg!(target_os = "linux") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let source_cu = manifest_dir.join("cuda/nv12_to_rgba8.cu");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let generated_ptx = out_dir.join("nv12_to_rgba8.ptx");

    println!("cargo:rerun-if-changed=cuda/nv12_to_rgba8.cu");
    println!("cargo:rerun-if-env-changed=CUDA_NVCC");
    println!("cargo:rerun-if-env-changed=LUMEN_CUDA_ARCH");

    compile_ptx(&source_cu, &generated_ptx).unwrap_or_else(|error| {
        panic!(
            "failed to compile CUDA kernel with nvcc: {error}\n\
             Install the CUDA toolkit (nvcc on PATH) or set CUDA_NVCC to the compiler path.\n\
             On Ubuntu: sudo apt-get install -y nvidia-cuda-toolkit"
        );
    });

    println!(
        "cargo:rustc-env=LUMEN_CUDA_NV12_PTX={}",
        generated_ptx.display()
    );
}

fn compile_ptx(source: &Path, output: &Path) -> Result<(), String> {
    if !source.is_file() {
        return Err(format!("CUDA source not found at {}", source.display()));
    }

    let nvcc = env::var("CUDA_NVCC").unwrap_or_else(|_| "nvcc".to_string());
    let arch = env::var("LUMEN_CUDA_ARCH").unwrap_or_else(|_| "sm_50".to_string());

    let status = Command::new(&nvcc)
        .args([
            "--ptx",
            source.to_str().expect("cuda source path"),
            "-o",
            output.to_str().expect("ptx output path"),
            &format!("-arch={arch}"),
        ])
        .status()
        .map_err(|error| format!("failed to run {nvcc}: {error}"))?;

    if !status.success() {
        return Err(format!("{nvcc} exited with {status}"));
    }

    Ok(())
}
