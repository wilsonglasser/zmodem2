// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2025 Jarkko Sakkinen

extern crate zmodem2;

use anyhow::{bail, Context};
use argh::FromArgs;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::File;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use zmodem2_bin::CombinedStdInOut;

#[derive(FromArgs, Debug)]
/// Receive a file using the ZMODEM protocol.
pub struct Arguments {
    /// optional path to save the file. if not given, saves in the current directory.
    #[argh(option, short = 'p')]
    pub path: Option<String>,
}

fn main() -> anyhow::Result<()> {
    let args: Arguments = argh::from_env();
    let mut port = CombinedStdInOut::new();
    let mut state = zmodem2::State::new();
    let mut buf = vec![];
    while state.stage() == zmodem2::Stage::Waiting {
        if zmodem2::receive(&mut port, &mut buf, &mut state).is_err() {
            bail!("connection lost");
        }
    }
    let received_filename = Path::new(state.file_name())
        .components()
        .last()
        .and_then(|c| match c {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .with_context(|| format!("invalid filename '{}'", state.file_name()))?;
    let path = match &args.path {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(received_filename),
    };
    let pb = ProgressBar::new(state.file_size() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{msg}\n{spinner} [{elapsed_precise}] [{bar:40}] {bytes}/{total_bytes} ({eta})",
            )?
            .progress_chars("=>-"),
    );
    pb.set_message(format!("Receiving {}", path.display()));
    let mut file =
        File::create(&path).with_context(|| format!("unable to create '{}'", path.display()))?;
    file.write_all(&buf)?;
    while state.stage() != zmodem2::Stage::Done {
        if let Err(e) = zmodem2::receive(&mut port, &mut file, &mut state) {
            pb.finish_with_message("Aborted");
            bail!("ZMODEM error: {:#}", e);
        }
        pb.set_position(state.count() as u64);
    }
    pb.finish_with_message("Done");
    Ok(())
}
