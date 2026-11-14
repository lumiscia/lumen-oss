use std::{fs, path::PathBuf};

/// The lavapipe Vulkan driver's ICD file, or `None` (after saying so) when it isn't installed.
/// With `LUMEN_REQUIRE_GPU=1` a missing driver fails the test instead.
pub fn lavapipe_icd() -> Option<PathBuf> {
    let icd = fs::read_dir("/usr/share/vulkan/icd.d")
        .into_iter()
        .flatten()
        .filter_map(|entry| Some(entry.ok()?.path()))
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("lvp_icd"))
        });
    if icd.is_none() {
        assert!(
            std::env::var_os("LUMEN_REQUIRE_GPU").is_none_or(|value| value != "1"),
            "a GPU is required (LUMEN_REQUIRE_GPU=1), but lavapipe is not installed"
        );
        eprintln!("lavapipe not installed; skipping");
    }
    icd
}
