use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const BRIDGE_HEADER: &str = "native/chip_bridge.h";
const BRIDGE_SOURCE: &str = "native/chip_bridge.cc";
const CHIP_EXAMPLE_STORAGE_SOURCE: &str = "src/controller/ExamplePersistentStorage.cpp";
const CHIP_FILE_ATTESTATION_TRUST_STORE_HEADER: &str =
    "src/credentials/attestation_verifier/FileAttestationTrustStore.h";
const CHIP_FILE_ATTESTATION_TRUST_STORE_SOURCE: &str =
    "src/credentials/attestation_verifier/FileAttestationTrustStore.cpp";

fn main() {
    println!("cargo:rustc-check-cfg=cfg(rhythm_chipd_chip_ffi)");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={BRIDGE_HEADER}");
    println!("cargo:rerun-if-changed={BRIDGE_SOURCE}");
    for key in [
        "RHYTHM_CHIP_ROOT",
        "RHYTHM_CHIP_OUT_DIR",
        "RHYTHM_CHIP_LIB_DIR",
        "RHYTHM_CHIP_CRYPTO",
        "RHYTHM_CHIP_SYSROOT",
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
                // Suppress cc's default `cargo:rustc-link-lib=stdc++` (dylib) so
                // we can pick the linkage mode ourselves — on musl targets we
                // static-link libstdc++ to avoid shipping a C++ runtime .so.
                .cpp_link_stdlib(None::<&str>)
                .file(BRIDGE_SOURCE)
                .file(artifacts.chip_root.join(CHIP_EXAMPLE_STORAGE_SOURCE))
                .file(
                    artifacts
                        .chip_root
                        .join(CHIP_FILE_ATTESTATION_TRUST_STORE_SOURCE),
                )
                .flag_if_supported("-std=c++17")
                // Build the bridge with the same RTTI assumptions as the
                // CHIP static library used by the rpiz toolchain. Otherwise
                // wrapper classes that derive from CHIP interfaces can emit
                // references to typeinfo symbols that libCHIP.a does not
                // provide.
                .flag_if_supported("-fno-rtti")
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
            build.define("RHYTHM_CHIP_BRIDGE_NATIVE_LIBCHIP", "1");

            build.compile("rhythm_chip_bridge");

            if let Err(error) = emit_native_chip_link_inputs(&artifacts) {
                println!(
                    "cargo:warning=chip-ffi requested, but native CHIP controller data-model objects are unavailable: {error}"
                );
                return;
            }

            println!("cargo:rustc-link-arg={}", artifacts.link_path.display());
            // Track libCHIP.a / individual CHIP object files so cargo reruns
            // this build script and re-links rhythm-chipd when any of them
            // changes (e.g. after a CHIP source edit + `ninja lib/libCHIP.a`).
            // Cargo treats link-args as opaque strings and does not track
            // files referenced through them on its own.
            println!("cargo:rerun-if-changed={}", artifacts.link_path.display());
            if let Ok(inputs) = artifacts.native_link_inputs() {
                for path in inputs {
                    println!("cargo:rerun-if-changed={}", path.display());
                }
            }
            for relative in [
                CHIP_EXAMPLE_STORAGE_SOURCE,
                CHIP_FILE_ATTESTATION_TRUST_STORE_HEADER,
                CHIP_FILE_ATTESTATION_TRUST_STORE_SOURCE,
            ] {
                println!(
                    "cargo:rerun-if-changed={}",
                    artifacts.chip_root.join(relative).display()
                );
            }
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
        println!("cargo:rustc-link-lib=c++");
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
        let chip_crypto = chip_crypto_backend(target);
        let mut link_args = vec!["-levent_core", "-levent_pthreads"];
        if chip_crypto != "mbedtls" {
            link_args.splice(0..0, ["-lssl", "-lcrypto"]);
        }
        // Rust links with -nodefaultlibs. Fold the C++ runtime and libgcc
        // support into the binary so we don't have to ship libstdc++.so.6 /
        // libgcc_s.so.1 on targets (e.g. Buildroot musl rootfs) that don't
        // install a C++ runtime by default. The `-l:<filename>.a` syntax
        // forces ld to the static archive regardless of -Bstatic/-Bdynamic
        // state, which `-static-libstdc++` does not reliably override when
        // the -lstdc++ reference comes from a prior rustc-link-lib. libatomic
        // covers the ARMv6 __sync_* builtins that the compiler emits as
        // libcalls when the arch has no native atomics.
        link_args.push("-l:libstdc++.a");
        link_args.push("-l:libgcc.a");
        link_args.push("-l:libgcc_eh.a");
        // libatomic is shadowed by Buildroot staging's soft-float build, so
        // link it dynamically — the hard-float libatomic.so.1 is on the
        // target rootfs and the runtime dynamic loader will find the right
        // ABI match.
        link_args.push("-latomic");
        for link_arg in link_args {
            println!("cargo:rustc-link-arg={link_arg}");
        }
    }
}

fn chip_crypto_backend(target: &str) -> String {
    if let Ok(value) = env::var("RHYTHM_CHIP_CRYPTO") {
        return value;
    }

    if target.contains("musl") {
        "mbedtls".to_string()
    } else {
        "openssl".to_string()
    }
}

fn add_platform_include_deps(target: &str, build: &mut cc::Build) -> Result<(), String> {
    if !target.contains("linux") {
        return Ok(());
    }

    let host = env::var("HOST").unwrap_or_default();
    if host != target {
        return add_cross_linux_include_deps(target, build);
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

fn add_cross_linux_include_deps(target: &str, build: &mut cc::Build) -> Result<(), String> {
    let sysroot = resolve_cross_sysroot(target).ok_or_else(|| {
        "pkg-config is unavailable for this cross-compile, and no sysroot could be resolved. Set RHYTHM_CHIP_SYSROOT or configure the target linker so `-print-sysroot` works.".to_string()
    })?;

    let mut added_any = false;
    for path in [
        sysroot.join("usr/include"),
        sysroot.join("usr/include/glib-2.0"),
        sysroot.join("usr/include/gio-unix-2.0"),
        sysroot.join("usr/include/dbus-1.0"),
        sysroot.join("usr/include/libmount"),
        sysroot.join("usr/include/blkid"),
    ] {
        if path.is_dir() {
            build.include(&path);
            added_any = true;
        }
    }

    for subdir in ["usr/lib", "usr/lib64", "lib", "lib64"] {
        let base = sysroot.join(subdir);
        add_nested_include_if_present(build, &base, "glib-2.0/include", &mut added_any);
        add_nested_include_if_present(build, &base, "dbus-1.0/include", &mut added_any);
    }

    if !added_any {
        return Err(format!(
            "resolved cross sysroot at {}, but no GLib/DBus include directories were found",
            sysroot.display()
        ));
    }

    emit_cross_linux_link_deps(&sysroot);

    Ok(())
}

fn emit_cross_linux_link_deps(sysroot: &Path) {
    for dir in collect_cross_link_dirs(sysroot) {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }

    // Emit as link-arg (not link-lib) so the libraries land at the end of the
    // linker command, after the CHIP object files and libCHIP.a. Force
    // --no-as-needed around the group so rustc's default `-Wl,--as-needed`
    // doesn't drop libs whose symbols are only referenced by later archives.
    // Allow undefined symbols in shared libs (e.g. glib's transitive pcre2/ffi/
    // zlib deps) — those resolve at runtime via the dynamic loader.
    println!("cargo:rustc-link-arg=-Wl,--allow-shlib-undefined");
    println!("cargo:rustc-link-arg=-Wl,--no-as-needed");
    for lib in [
        "gio-2.0",
        "gobject-2.0",
        "glib-2.0",
        "dbus-1",
        "avahi-client",
        "avahi-common",
    ] {
        println!("cargo:rustc-link-arg=-l{lib}");
    }
    println!("cargo:rustc-link-arg=-Wl,--as-needed");
}

fn collect_cross_link_dirs(sysroot: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    for subdir in ["usr/lib", "usr/lib64", "lib", "lib64"] {
        let base = sysroot.join(subdir);
        push_link_dir(&mut dirs, &base);

        let Ok(entries) = fs::read_dir(&base) else {
            continue;
        };

        for entry in entries.flatten() {
            push_link_dir(&mut dirs, &entry.path());
        }
    }

    dirs
}

fn push_link_dir(dirs: &mut Vec<PathBuf>, path: &Path) {
    if path.is_dir() && !dirs.iter().any(|existing| existing == path) {
        dirs.push(path.to_path_buf());
    }
}

fn add_nested_include_if_present(
    build: &mut cc::Build,
    base: &Path,
    suffix: &str,
    added_any: &mut bool,
) {
    if !base.is_dir() {
        return;
    }

    let direct = base.join(suffix);
    if direct.is_dir() {
        build.include(&direct);
        *added_any = true;
    }

    let Ok(entries) = fs::read_dir(base) else {
        return;
    };

    for entry in entries.flatten() {
        let candidate = entry.path().join(suffix);
        if candidate.is_dir() {
            build.include(&candidate);
            *added_any = true;
        }
    }
}

fn resolve_cross_sysroot(target: &str) -> Option<PathBuf> {
    if let Some(sysroot) = env::var_os("RHYTHM_CHIP_SYSROOT").map(PathBuf::from) {
        return Some(sysroot);
    }

    for program in cross_compiler_candidates(target) {
        let Ok(output) = Command::new(&program).arg("-print-sysroot").output() else {
            continue;
        };
        if !output.status.success() {
            continue;
        }

        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if value.is_empty() {
            continue;
        }

        let path = PathBuf::from(value);
        if path.is_dir() {
            return Some(path);
        }
    }

    None
}

fn cross_compiler_candidates(target: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let target_upper = target.replace('-', "_").to_ascii_uppercase();
    let target_lower = target.replace('-', "_");

    for key in [
        format!("CARGO_TARGET_{target_upper}_LINKER"),
        format!("CC_{target_lower}"),
        "CC".to_string(),
    ] {
        if let Ok(value) = env::var(&key) {
            if !value.trim().is_empty() && !candidates.iter().any(|candidate| candidate == &value) {
                candidates.push(value);
            }
        }
    }

    candidates
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
            out_dir,
            target,
            link_path: libchip,
        });
    }

    Err(format!(
        "no native libCHIP.a found under {}",
        out_dir.display()
    ))
}

fn resolve_from_lib_dir(lib_dir: PathBuf, target: String) -> Result<ChipArtifacts, String> {
    let chip_root = infer_root_from_lib_dir(&lib_dir)
        .ok_or_else(|| format!("unable to infer CHIP root from {}", lib_dir.display()))?;
    let out_dir = infer_out_dir_from_lib_dir(&lib_dir)
        .ok_or_else(|| format!("unable to infer CHIP out dir from {}", lib_dir.display()))?;

    let libchip = lib_dir.join("libCHIP.a");
    if libchip.is_file() {
        return Ok(ChipArtifacts {
            chip_root,
            out_dir,
            target,
            link_path: libchip,
        });
    }

    Err(format!("no libCHIP.a found under {}", lib_dir.display()))
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

fn infer_out_dir_from_lib_dir(lib_dir: &Path) -> Option<PathBuf> {
    let parent = lib_dir.parent()?;
    if lib_dir.file_name()? == "lib" && parent.join("gen/include").is_dir() {
        return Some(parent.to_path_buf());
    }

    for ancestor in lib_dir.ancestors() {
        if ancestor.join("gen/include").is_dir() && ancestor.join("obj").is_dir() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn emit_native_chip_link_inputs(artifacts: &ChipArtifacts) -> Result<(), String> {
    for path in artifacts.native_link_inputs()? {
        println!("cargo:rustc-link-arg={}", path.display());
    }
    Ok(())
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
    out_dir: PathBuf,
    target: String,
    link_path: PathBuf,
}

impl ChipArtifacts {
    fn include_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = vec![
            self.chip_root.join("src/include"),
            self.chip_root.join("src"),
            self.out_dir.join("gen/include"),
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

    fn native_link_inputs(&self) -> Result<Vec<PathBuf>, String> {
        const REQUIRED_OBJECTS: &[&str] = &[
            "obj/zzz_generated/app-common/app-common/zap-generated/attributes/data_model.Accessors.cpp.o",
            "obj/src/data-model-providers/codegen/data_model.ClusterIntegration.cpp.o",
            "obj/src/data-model-providers/codegen/data_model.CodegenDataModelProvider.cpp.o",
            "obj/src/data-model-providers/codegen/data_model.CodegenDataModelProvider_Read.cpp.o",
            "obj/src/data-model-providers/codegen/data_model.CodegenDataModelProvider_Write.cpp.o",
            "obj/src/data-model-providers/codegen/data_model.EmberAttributeDataBuffer.cpp.o",
            "obj/src/data-model-providers/codegen/data_model.Instance.cpp.o",
            "obj/src/app/util/data_model.generic-callback-stubs.cpp.o",
            "obj/src/app/util/data_model.privilege-storage.cpp.o",
            "obj/src/app/reporting/data_model.reporting.cpp.o",
            "obj/src/app/util/data_model.DataModelHandler.cpp.o",
            "obj/src/app/util/data_model.attribute-storage.cpp.o",
            "obj/src/app/util/data_model.attribute-table.cpp.o",
            "obj/src/app/util/data_model.ember-io-storage.cpp.o",
            "obj/src/app/util/data_model.util.cpp.o",
            "obj/src/app/server-cluster/server-cluster.AttributeListBuilder.cpp.o",
            "obj/src/app/server-cluster/server-cluster.DefaultServerCluster.cpp.o",
            "obj/src/app/server-cluster/server-cluster.ServerClusterExtension.cpp.o",
            "obj/src/app/server-cluster/server-cluster.ServerClusterInterface.cpp.o",
            "obj/src/app/server-cluster/registry.ServerClusterInterfaceRegistry.cpp.o",
            "obj/src/app/server-cluster/registry.SingleEndpointServerClusterRegistry.cpp.o",
            "obj/BUILD_DIR/gen/src/controller/data_model/app/data_model_codegen.callback-stub.cpp.o",
            "obj/BUILD_DIR/gen/src/controller/data_model/app/data_model_codegen.cluster-callbacks.cpp.o",
            "obj/BUILD_DIR/gen/src/controller/data_model/zapgen/zap-generated/data_model_zapgen.CodeDrivenInitShutdown.cpp.o",
            "obj/BUILD_DIR/gen/src/controller/data_model/zapgen/zap-generated/data_model_zapgen.IMClusterCommandHandler.cpp.o",
            "obj/src/app/persistence/persistence.AttributePersistence.cpp.o",
            "obj/src/app/persistence/persistence.String.cpp.o",
            "obj/src/app/persistence/default.DefaultAttributePersistenceProvider.cpp.o",
            "obj/src/app/persistence/singleton.AttributePersistenceProviderInstance.cpp.o",
        ];

        let mut inputs = Vec::with_capacity(REQUIRED_OBJECTS.len());
        for relative in REQUIRED_OBJECTS {
            let path = self.out_dir.join(relative);
            if !path.is_file() {
                return Err(format!(
                    "expected CHIP object '{}' under {}",
                    relative,
                    self.out_dir.display()
                ));
            }
            inputs.push(path);
        }

        Ok(inputs)
    }
}
