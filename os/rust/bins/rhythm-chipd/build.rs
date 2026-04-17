use std::env;
use std::path::{Path, PathBuf};

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

    match resolve_chip_artifacts() {
        Ok(artifacts) => {
            let mut build = cc::Build::new();
            build
                .cpp(true)
                .file(BRIDGE_SOURCE)
                .file(
                    artifacts
                        .chip_root
                        .join("src/controller/ExamplePersistentStorage.cpp"),
                )
                .flag_if_supported("-std=c++17")
                .warnings(false)
                .define("CHIP_HAVE_CONFIG_H", "1")
                .define("OPENSSL_NO_ASM", "1");

            for include_dir in artifacts.include_dirs() {
                build.include(include_dir);
            }
            if let Err(error) = add_platform_include_deps(&artifacts.target, &mut build) {
                println!(
                    "cargo:warning=chip-ffi requested, but Linux system headers are unavailable: {error}"
                );
                return;
            }
            match artifacts.link_mode {
                LinkMode::NativeLibChip => {
                    build.define("RHYTHM_CHIP_BRIDGE_NATIVE_LIBCHIP", "1");
                }
                LinkMode::PythonExtension => {
                    build.define("RHYTHM_CHIP_BRIDGE_PYTHON_EXTENSION", "1");
                    build.include(artifacts.chip_root.join("src/controller/python"));
                }
            }

            build.compile("rhythm_chip_bridge");

            println!("cargo:rustc-link-arg={}", artifacts.link_path.display());
            emit_platform_link_args(&artifacts.target);
            println!("cargo:rustc-cfg=rhythm_chipd_chip_ffi");
        }
        Err(error) => {
            println!(
                "cargo:warning=chip-ffi requested, but direct CHIP bridge is disabled: {error}"
            );
        }
    }
}

fn emit_platform_link_args(target: &str) {
    if target.contains("apple-darwin") {
        for framework in [
            "CoreData",
            "CoreFoundation",
            "CoreBluetooth",
            "Foundation",
            "Network",
            "SystemConfiguration",
            "CoreWLAN",
            "IOKit",
        ] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    } else if target.contains("linux") {
        for link_arg in ["-lssl", "-lcrypto", "-levent_core", "-levent_pthreads"] {
            println!("cargo:rustc-link-arg={link_arg}");
        }
    }
}

fn add_platform_include_deps(target: &str, build: &mut cc::Build) -> Result<(), String> {
    if !target.contains("linux") {
        return Ok(());
    }

    for package in [
        "gio-2.0",
        "glib-2.0",
        "gobject-2.0",
        "dbus-1",
        "avahi-client",
    ] {
        let library = pkg_config::Config::new()
            .cargo_metadata(true)
            .probe(package)
            .map_err(|error| {
                format!(
                    "pkg-config could not resolve {package}: {error}. Install the Linux CHIP build dependencies, especially libglib2.0-dev, libdbus-1-dev, and libavahi-client-dev."
                )
            })?;

        for include_path in library.include_paths {
            build.include(include_path);
        }
    }

    Ok(())
}

fn resolve_chip_artifacts() -> Result<ChipArtifacts, String> {
    let host = env::var("HOST").unwrap_or_default();
    let target = env::var("TARGET").unwrap_or_default();

    if let Some(lib_dir) = env::var_os("RHYTHM_CHIP_LIB_DIR").map(PathBuf::from) {
        return resolve_from_lib_dir(lib_dir, target);
    }

    if let Some(out_dir) = env::var_os("RHYTHM_CHIP_OUT_DIR").map(PathBuf::from) {
        return resolve_from_out_dir(out_dir, target);
    }

    if host != target {
        return Err(
            "cross-compiles need RHYTHM_CHIP_OUT_DIR or RHYTHM_CHIP_LIB_DIR pointing at target-specific connectedhomeip artifacts"
                .to_string(),
        );
    }

    if let Some(root) = env::var_os("RHYTHM_CHIP_ROOT").map(PathBuf::from) {
        return resolve_from_root(root, target);
    }

    for root in inferred_chip_roots() {
        if let Ok(artifacts) = resolve_from_root(root, target.clone()) {
            return Ok(artifacts);
        }
    }

    Err(
        "set RHYTHM_CHIP_ROOT, RHYTHM_CHIP_OUT_DIR, or RHYTHM_CHIP_LIB_DIR before enabling chip-ffi"
            .to_string(),
    )
}

fn resolve_from_root(chip_root: PathBuf, target: String) -> Result<ChipArtifacts, String> {
    resolve_from_out_dir_with_root(chip_root.join("out/host"), chip_root, target)
}

fn resolve_from_out_dir(out_dir: PathBuf, target: String) -> Result<ChipArtifacts, String> {
    let chip_root = out_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| format!("unable to determine CHIP root from {}", out_dir.display()))?
        .to_path_buf();

    resolve_from_out_dir_with_root(out_dir, chip_root, target)
}

fn resolve_from_out_dir_with_root(
    out_dir: PathBuf,
    chip_root: PathBuf,
    target: String,
) -> Result<ChipArtifacts, String> {
    let chip_root = chip_root.canonicalize().unwrap_or(chip_root);
    let out_dir = out_dir.canonicalize().unwrap_or(out_dir);

    let libchip = out_dir.join("lib/libCHIP.a");
    if libchip.is_file() {
        return Ok(ChipArtifacts {
            chip_root,
            target,
            link_path: libchip,
            link_mode: LinkMode::NativeLibChip,
        });
    }

    let python_extension = out_dir.join("obj/src/controller/python/matter/_ChipDeviceCtrl.so");
    if python_extension.is_file() {
        return Ok(ChipArtifacts {
            chip_root,
            target,
            link_path: python_extension,
            link_mode: LinkMode::PythonExtension,
        });
    }

    Err(format!(
        "no native libCHIP.a or _ChipDeviceCtrl.so found under {}",
        out_dir.display()
    ))
}

fn resolve_from_lib_dir(lib_dir: PathBuf, target: String) -> Result<ChipArtifacts, String> {
    let chip_root = infer_root_from_lib_dir(&lib_dir)
        .ok_or_else(|| format!("unable to infer CHIP root from {}", lib_dir.display()))?;

    let libchip = lib_dir.join("libCHIP.a");
    if libchip.is_file() {
        return Ok(ChipArtifacts {
            chip_root,
            target,
            link_path: libchip,
            link_mode: LinkMode::NativeLibChip,
        });
    }

    let python_extension = lib_dir.join("_ChipDeviceCtrl.so");
    if python_extension.is_file() {
        return Ok(ChipArtifacts {
            chip_root,
            target,
            link_path: python_extension,
            link_mode: LinkMode::PythonExtension,
        });
    }

    Err(format!(
        "no libCHIP.a or _ChipDeviceCtrl.so found under {}",
        lib_dir.display()
    ))
}

fn infer_root_from_lib_dir(lib_dir: &Path) -> Option<PathBuf> {
    let parent = lib_dir.parent()?;
    if parent.file_name()? == "host" && lib_dir.file_name()? == "lib" {
        return parent.parent().map(Path::to_path_buf);
    }

    for ancestor in lib_dir.ancestors() {
        if ancestor.join("src").is_dir() && ancestor.join("config/standalone").is_dir() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
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

struct ChipArtifacts {
    chip_root: PathBuf,
    target: String,
    link_path: PathBuf,
    link_mode: LinkMode,
}

impl ChipArtifacts {
    fn include_dirs(&self) -> Vec<PathBuf> {
        let out_dir = self
            .link_path
            .ancestors()
            .find(|path| path.file_name().is_some_and(|name| name == "host"))
            .or_else(|| {
                self.link_path
                    .ancestors()
                    .find(|path| path.join("gen/include").is_dir())
            })
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.chip_root.join("out/host"));

        let mut dirs = vec![
            self.chip_root.join("src/include"),
            self.chip_root.join("src"),
            out_dir.join("gen/include"),
            self.chip_root.join("config/standalone"),
            self.chip_root.join("zzz_generated/app-common"),
            self.chip_root.join("third_party/nlassert/repo/include"),
            self.chip_root.join("third_party/nlio/repo/include"),
            self.chip_root.join("third_party/nlfaultinjection/include"),
            self.chip_root.join("third_party/inipp/repo/inipp"),
            self.chip_root
                .join("third_party/ot-commissioner/repo/include"),
            self.chip_root.join("third_party/ot-commissioner/repo/src"),
            self.chip_root
                .join("third_party/ot-commissioner/repo/third_party/fmtlib/repo/include"),
            self.chip_root
                .join("third_party/ot-commissioner/repo/third_party/cn-cbor/repo/include"),
            self.chip_root
                .join("third_party/ot-commissioner/repo/third_party/COSE-C/repo/include"),
            self.chip_root
                .join("third_party/ot-commissioner/repo/third_party/json/repo/single_include"),
            self.chip_root
                .join("third_party/ot-commissioner/repo/third_party/mbedtls/repo/include"),
            self.chip_root
                .join("third_party/ot-commissioner/repo/third_party/mdns/repo/include"),
            self.chip_root
                .join("third_party/boringssl/repo/src/include"),
        ];

        if self.target.contains("apple-darwin") {
            dirs.push(self.chip_root.join("src/tracing/darwin/include"));
        }

        dirs
    }
}

enum LinkMode {
    NativeLibChip,
    PythonExtension,
}
