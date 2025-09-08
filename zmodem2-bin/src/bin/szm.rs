// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2025 Jarkko Sakkinen

extern crate zmodem2;

use anyhow::{bail, Context};
use argh::FromArgs;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::File;
use std::path::Path;
use zmodem2_bin::CombinedStdInOut;

#[derive(FromArgs, Debug)]
/// Send a file using the ZMODEM protocol.
pub struct Arguments {
    /// path of the file to send
    #[argh(option, short = 'p')]
    pub path: String,
}

fn main() -> anyhow::Result<()> {
    let args: Arguments = argh::from_env();
    let mut file = File::open(&args.path)
        .with_context(|| format!("unable to open '{}'", args.path))?;
    let filename_path = Path::new(&args.path);
    let filename = filename_path
        .file_name()
        .with_context(|| format!("unable to extract filename from '{}'", args.path))?;
    let filename_str = filename
        .to_str()
        .context("filename is not valid UTF-8 string")?;
    let size = file.metadata()?.len();
    let mut port = CombinedStdInOut::new();
    let mut state = zmodem2::State::new_file(filename_str, size as u32)?;
    let pb = ProgressBar::new(size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{msg}\n{spinner} [{elapsed_precise}] [{bar:40}] {bytes}/{total_bytes} ({eta})",
            )?
            .progress_chars("=>-"),
    );
    pb.set_message(format!("Sending {}", filename_str));
    while state.stage() != zmodem2::Stage::Done {
        if let Err(e) = zmodem2::send(&mut port, &mut file, &mut state) {
            pb.finish_with_message("Aborted");
            bail!("ZMODEM error: {:#}", e);
        }
        pb.set_position(state.count() as u64);
    }
    pb.finish_with_message("Done");
    Ok(())
}
