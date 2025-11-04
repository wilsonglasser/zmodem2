// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2025 Jarkko Sakkinen

extern crate zmodem2;

use anyhow::{bail, Context};
use argh::FromArgs;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::{File, OpenOptions};
use std::path::Path;
use zmodem2_bin::ReadWrite;

#[derive(FromArgs, Debug)]
/// Sends files using the ZMODEM protocol.
struct Arguments {
    /// device file [default: /dev/ttyS0]
    #[argh(option, short = 'p', default = "String::from(\"/dev/ttyS0\")")]
    port: String,
    /// list of files
    #[argh(positional)]
    paths: Vec<String>,
}

fn main() -> anyhow::Result<()> {
    let args: Arguments = argh::from_env();
    if args.paths.is_empty() {
        std::process::exit(2);
    }

    let mut port: Box<dyn ReadWrite> = Box::new(
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&args.port)
            .with_context(|| format!("'{}'", args.port))?,
    );
    let mut state = zmodem2::State::new();
    for path in &args.paths {
        let mut file = File::open(path).with_context(|| format!("'{path}'"))?;
        let filename_path = Path::new(path);
        let filename = filename_path
            .file_name()
            .with_context(|| format!("'{}'", path))?;
        let filename_str = filename.to_str().context("invalid UTF-8")?;
        let size = file.metadata()?.len();
        state = zmodem2::State::new_file(filename_str, size as u32)?;
        let pb = ProgressBar::new(size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template(
                    "{msg}\n{spinner} [{elapsed_precise}] [{bar:40}] {bytes}/{total_bytes} ({eta})",
                )?
                .progress_chars("=>-"),
        );
        pb.set_message(format!("Sending {}", filename_str));
        while state.stage() != zmodem2::Stage::FileEnd {
            if let Err(e) = zmodem2::send(&mut port, &mut file, &mut state) {
                pb.finish_with_message("Aborted");
                bail!("ZMODEM error: {:#}", e);
            }
            pb.set_position(state.count() as u64);
        }
        pb.finish_with_message(format!("Sent {}", filename_str));
    }
    while state.stage() != zmodem2::Stage::SessionEnd {
        if let Err(e) = zmodem2::finish(&mut port, &mut state) {
            bail!("ZMODEM error during finish: {:#}", e);
        }
    }
    Ok(())
}
