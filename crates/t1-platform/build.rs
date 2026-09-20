use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=CC");
    println!("cargo:rerun-if-env-changed=AR");
    let manifest = PathBuf::from(required_env("CARGO_MANIFEST_DIR"));
    let output = PathBuf::from(required_env("OUT_DIR"));
    let compiler = env::var_os("CC").unwrap_or_else(|| "cc".into());
    let archiver = env::var_os("AR").unwrap_or_else(|| "ar".into());
    let mut objects = Vec::new();

    if env::var_os("CARGO_FEATURE_FRAME_MEMFD").is_some() {
        objects.push(compile(
            &compiler,
            &manifest.join("c/t1_frame_memfd.c"),
            &output.join("t1_frame_memfd.o"),
        ));
    }
    if env::var_os("CARGO_FEATURE_SECRET_WIPE").is_some() {
        objects.push(compile(
            &compiler,
            &manifest.join("c/t1_secret_wipe.c"),
            &output.join("t1_secret_wipe.o"),
        ));
    }
    if env::var_os("CARGO_FEATURE_SEP_OPERATION").is_some() {
        compile_sep_operation(&mut objects, &compiler, &manifest, &output);
    }
    if env::var_os("CARGO_FEATURE_IMPORT_FS").is_some() {
        objects.push(compile(
            &compiler,
            &manifest.join("../t1-import/c/t1_import_fs.c"),
            &output.join("t1_import_fs.o"),
        ));
    }
    if env::var_os("CARGO_FEATURE_PRESERVED_EFI").is_some() {
        objects.push(compile(
            &compiler,
            &manifest.join("../t1-import/c/t1_preserved_efi.c"),
            &output.join("t1_preserved_efi.o"),
        ));
    }
    let preserved_efi_discovery = env::var_os("CARGO_FEATURE_PRESERVED_EFI_DISCOVERY").is_some();
    if preserved_efi_discovery {
        objects.push(compile(
            &compiler,
            &manifest.join("../t1-import/c/t1_efi_roots.c"),
            &output.join("t1_efi_roots.o"),
        ));
    }
    if env::var_os("CARGO_FEATURE_SEQPACKET").is_some() {
        objects.push(compile(
            &compiler,
            &manifest.join("../t1-daemons/c/t1_seqpacket.c"),
            &output.join("t1_seqpacket.o"),
        ));
    }
    if env::var_os("CARGO_FEATURE_TOUCHBAR_DRM").is_some() {
        objects.push(compile(
            &compiler,
            &manifest.join("c/t1_touchbar_drm.c"),
            &output.join("t1_touchbar_drm.o"),
        ));
    }
    if env::var_os("CARGO_FEATURE_TOUCHBAR_IO").is_some() {
        for source in [
            "t1_touchbar_io.c",
            "t1_touchbar_digitizer.c",
            "t1_touchbar_fn.c",
            "t1_touchbar_uinput.c",
        ] {
            objects.push(compile(
                &compiler,
                &manifest.join("../t1-touchbar-hw/c").join(source),
                &output.join(source.replace(".c", ".o")),
            ));
        }
    }
    let touchbar_session = env::var_os("CARGO_FEATURE_TOUCHBAR_SESSION").is_some();
    if touchbar_session {
        objects.push(compile(
            &compiler,
            &manifest.join("c/t1_touchbar_session.c"),
            &output.join("t1_touchbar_session.o"),
        ));
    }
    compile_usb_cycle_guard(&mut objects, &compiler, &manifest, &output);
    compile_recovery(&mut objects, &compiler, &manifest, &output);
    let xz = env::var_os("CARGO_FEATURE_XZ").is_some();
    if xz {
        objects.push(compile(
            &compiler,
            &manifest.join("../t1-import/c/t1_xz.c"),
            &output.join("t1_xz.o"),
        ));
    }

    link_native(
        &objects,
        &archiver,
        &output,
        xz,
        touchbar_session,
        preserved_efi_discovery,
    );
}

fn compile_recovery(objects: &mut Vec<PathBuf>, compiler: &OsStr, manifest: &Path, output: &Path) {
    if env::var_os("CARGO_FEATURE_ONLINE_RECOVERY").is_none() {
        return;
    }
    for name in ["t1_recovery_fs", "t1_recovery_io"] {
        objects.push(compile(
            compiler,
            &manifest.join(format!("c/{name}.c")),
            &output.join(format!("{name}.o")),
        ));
    }
    for library in ["curl", "archive", "crypto", "udev"] {
        println!("cargo:rustc-link-lib={library}");
    }
}

fn compile_usb_cycle_guard(
    objects: &mut Vec<PathBuf>,
    compiler: &OsStr,
    manifest: &Path,
    output: &Path,
) {
    if env::var_os("CARGO_FEATURE_USB_CYCLE_GUARD").is_some() {
        objects.push(compile(
            compiler,
            &manifest.join("c/t1_usb_cycle_guard.c"),
            &output.join("t1_usb_cycle_guard.o"),
        ));
    }
}

fn link_native(
    objects: &[PathBuf],
    archiver: &OsStr,
    output: &Path,
    xz: bool,
    touchbar_session: bool,
    preserved_efi_discovery: bool,
) {
    if !objects.is_empty() {
        let archive = output.join("libt1_platform_native.a");
        let _ = fs::remove_file(&archive);
        let mut command = Command::new(archiver);
        command.arg("rcs").arg(&archive).args(objects);
        run(&mut command, "C archive");
        println!("cargo:rustc-link-search=native={}", output.display());
        println!("cargo:rustc-link-lib=static=t1_platform_native");
    }
    if xz {
        println!("cargo:rustc-link-lib=lzma");
    }
    if touchbar_session {
        println!("cargo:rustc-link-lib=systemd");
    }
    if preserved_efi_discovery {
        println!("cargo:rustc-link-lib=udev");
    }
}

fn compile_sep_operation(
    objects: &mut Vec<PathBuf>,
    compiler: &OsStr,
    manifest: &Path,
    output: &Path,
) {
    for source in [
        "sep_crypto.c",
        "sep_relay.c",
        "sep_usbfs.c",
        "sep_urb.c",
        "sep_acm.c",
        "sep_keystore.c",
        "sep_session.c",
        "sep_operation.c",
        "sep_keybag_store.c",
        "sep_keybag.c",
    ] {
        objects.push(compile(
            compiler,
            &manifest.join("../../sep-probe").join(source),
            &output.join(source.replace(".c", ".o")),
        ));
    }
}

fn compile(compiler: &OsStr, source: &Path, object: &Path) -> PathBuf {
    println!("cargo:rerun-if-changed={}", source.display());
    if let Some(header) = source.with_extension("h").to_str() {
        println!("cargo:rerun-if-changed={header}");
    }
    let mut command = Command::new(compiler);
    command.args([
        OsStr::new("-O2"),
        OsStr::new("-std=c17"),
        OsStr::new("-Wall"),
        OsStr::new("-Wextra"),
        OsStr::new("-Wpedantic"),
        OsStr::new("-Werror"),
        OsStr::new("-fPIC"),
        OsStr::new("-c"),
    ]);
    command.arg(source).arg("-o").arg(object);
    run(&mut command, "C compilation");
    object.to_owned()
}

fn run(command: &mut Command, operation: &str) {
    let status = command
        .status()
        .unwrap_or_else(|_| panic!("{operation} tool could not be started"));
    assert!(status.success(), "{operation} failed");
}

fn required_env(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("Cargo did not provide {name}"))
}
