use std::env;
use std::path::PathBuf;

const CHIP_LIB_SUBDIR: &str = "obj/src/controller/python/matter";
const BRIDGE_HEADER: &str = "native/chip_bridge.h";
const BRIDGE_SOURCE: &str = "native/chip_bridge.cc";

fn main() {
    println!("cargo:rustc-check-cfg=cfg(rhythm_chipd_chip_ffi)");
    println!("cargo:rerun-if-changed={BRIDGE_HEADER}");
    println!("cargo:rerun-if-changed={BRIDGE_SOURCE}");
    for key in [
        "RHYTHM_CHIP_ROOT",
        "RHYTHM_CHIP_OUT_DIR",
        "RHYTHM_CHIP_LIB_DIR",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }

    if env::var_os("CARGO_FEATURE_CHIP_FFI").is_none() {
        return;
    }

    match resolve_chip_lib_dir() {
        Ok(chip_library) => {
            cc::Build::new()
                .cpp(true)
                .file(BRIDGE_SOURCE)
                .flag_if_supported("-std=c++17")
                .compile("rhythm_chip_bridge");

            println!(
                "cargo:rustc-link-arg={}",
                chip_library.library_path.display()
            );
            println!("cargo:rustc-cfg=rhythm_chipd_chip_ffi");
        }
        Err(error) => {
            println!(
                "cargo:warning=chip-ffi requested, but direct CHIP bridge is disabled: {error}"
            );
        }
    }
}

fn resolve_chip_lib_dir() -> Result<ChipLibrary, String> {
    if let Some(lib_dir) = env::var_os("RHYTHM_CHIP_LIB_DIR").map(PathBuf::from) {
        return require_chip_library(lib_dir);
    }

    if let Some(out_dir) = env::var_os("RHYTHM_CHIP_OUT_DIR").map(PathBuf::from) {
        return require_chip_library(out_dir.join(CHIP_LIB_SUBDIR));
    }

    let host = env::var("HOST").unwrap_or_default();
    let target = env::var("TARGET").unwrap_or_default();
    if host != target {
        return Err(
            "cross-compiles need RHYTHM_CHIP_OUT_DIR or RHYTHM_CHIP_LIB_DIR pointing at target-specific connectedhomeip artifacts"
                .to_string(),
        );
    }

    if let Some(root) = env::var_os("RHYTHM_CHIP_ROOT").map(PathBuf::from) {
        return require_chip_library(root.join("out/host").join(CHIP_LIB_SUBDIR));
    }

    for root in inferred_chip_roots() {
        let candidate = root.join("out/host").join(CHIP_LIB_SUBDIR);
        if let Ok(lib_dir) = require_chip_library(candidate) {
            return Ok(lib_dir);
        }
    }

    Err(
        "set RHYTHM_CHIP_ROOT, RHYTHM_CHIP_OUT_DIR, or RHYTHM_CHIP_LIB_DIR before enabling chip-ffi"
            .to_string(),
    )
}

fn require_chip_library(lib_dir: PathBuf) -> Result<ChipLibrary, String> {
    let candidates = [
        "_ChipDeviceCtrl.so",
        "_ChipDeviceCtrl.dylib",
        "_ChipDeviceCtrl.dll",
        "lib_ChipDeviceCtrl.so",
        "lib_ChipDeviceCtrl.dylib",
    ];
    for name in candidates {
        let library_path = lib_dir.join(name);
        if library_path.is_file() {
            return Ok(ChipLibrary { library_path });
        }
    }

    Err(format!(
        "no _ChipDeviceCtrl shared library found under {}",
        lib_dir.display()
    ))
}

fn inferred_chip_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let current_dir = env::current_dir().ok();

    for base in [Some(manifest_dir), current_dir].into_iter().flatten() {
        for ancestor in base.ancestors() {
            for suffix in ["connectedhomeip", "rhythm/connectedhomeip"] {
                let candidate = ancestor.join(suffix);
                if candidate.is_dir() && !roots.iter().any(|root| root == &candidate) {
                    roots.push(candidate);
                }
            }
        }
    }

    roots
}

struct ChipLibrary {
    library_path: PathBuf,
}
