// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2025 Jarkko Sakkinen

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=OUT_DIR");
    println!("cargo:rustc-check-cfg=cfg(host_has_rzsz)");

    // rzsz
    if Command::new("rz").spawn().is_ok() && Command::new("sz").spawn().is_ok() {
        println!("cargo:rustc-cfg=host_has_rzsz");
    } else {
        println!("cargo:warning=no rzsz");
    }
}
