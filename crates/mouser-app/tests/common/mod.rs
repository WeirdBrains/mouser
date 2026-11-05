//! Shared test helpers.

use std::path::{Path, PathBuf};

/// Stage Mouser's own engine libraries (`mouser` = wgpu, `skia`), which cargo
/// builds next to the test binary as dev-dependencies, into `into` laid out
/// like `engines/` beside a shipped Mouser. Point `MOUSER_ENGINE_DIRS` at it.
pub fn official_engines(into: &Path) -> PathBuf {
    let deps = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
    for stem in ["mouser_engine_wgpu", "mouser_engine_skia"] {
        let lib = deps.join(mouser_core::engines::lib_file(stem));
        assert!(lib.is_file(), "{} not built (a dev-dependency of mouser-app)", lib.display());
        mouser_core::engines::stage(&lib, into).unwrap();
    }
    into.to_path_buf()
}
