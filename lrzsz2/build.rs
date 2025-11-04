// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2025 Jarkko Sakkinen

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=OUT_DIR");
    println!("cargo:rustc-check-cfg=cfg(host_has_rzsz)");
    println!("cargo:rustc-check-cfg=cfg(feature, values(\"integration_test\"))");

    let rz_exists = Command::new("sh")
        .arg("-c")
        .arg("command -v rz")
        .status()
        .is_ok_and(|s| s.success());

    let sz_exists = Command::new("sh")
        .arg("-c")
        .arg("command -v sz")
        .status()
        .is_ok_and(|s| s.success());

    if rz_exists && sz_exists {
        println!("cargo:rustc-cfg=host_has_rzsz");
    } else {
        println!("cargo:warning=rz/sz not found in PATH, skipping integration tests");
    }
}
