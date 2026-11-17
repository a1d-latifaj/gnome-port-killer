use std::{env, fs, path::PathBuf, process::Command};
fn main() {
    println!("cargo:rerun-if-changed=bpf/monitor.bpf.c");
    println!("cargo:rerun-if-env-changed=PORT_KILLER_BTF");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let btf = env::var("PORT_KILLER_BTF").unwrap_or_else(|_| "/sys/kernel/btf/vmlinux".into());
    let header = Command::new("bpftool")
        .args(["btf", "dump", "file", &btf, "format", "c"])
        .output()
        .expect("Building requires bpftool (runtime does not)");
    assert!(
        header.status.success(),
        "bpftool: {}",
        String::from_utf8_lossy(&header.stderr)
    );
    fs::write(out.join("vmlinux.h"), header.stdout).unwrap();
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "x86_64" => "-D__TARGET_ARCH_x86",
        "aarch64" => "-D__TARGET_ARCH_arm64",
        other => panic!("Unsupported build architecture: {other}"),
    };
    let status = Command::new("clang")
        .args(["-target", "bpf", "-O2", "-g", "-Wall", "-Werror", arch])
        .arg("-I")
        .arg(&out)
        .args(["-c", "bpf/monitor.bpf.c", "-o"])
        .arg(out.join("monitor.bpf.o"))
        .status()
        .expect("Building requires clang with the BPF target");
    assert!(status.success(), "BPF compilation failed");
}
