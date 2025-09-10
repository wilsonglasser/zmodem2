// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2025 Jarkko Sakkinen

extern crate zmodem2;

use anyhow::{bail, Context};
use argh::FromArgs;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use zmodem2_bin::{CombinedStdInOut, ReadWrite};

#[derive(FromArgs, Debug)]
/// Receive files using the ZMODEM protocol.
struct Arguments {
    /// default is '/dev/ttyS0'. Use '-' for stdio.
    #[argh(option, short = 'p')]
    port: Option<String>,
    /// download path
    #[argh(positional)]
    path: Option<String>,
}

fn main() -> anyhow::Result<()> {
    let args: Arguments = argh::from_env();
    let mut port: Box<dyn ReadWrite> = {
        let path = args.port.as_deref().unwrap_or("/dev/ttyS0");
        if path == "-" {
            Box::new(CombinedStdInOut::new())
        } else {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .with_context(|| format!("'{path}'"))?;
            Box::new(file)
        }
    };

    let mut state = zmodem2::State::new();
    let mut buf = vec![];
    while state.stage() == zmodem2::Stage::SessionBegin {
        if zmodem2::receive(&mut port, &mut buf, &mut state).is_err() {
            bail!("connection lost");
        }
    }
    let received_filename = Path::new(state.file_name())
        .components()
        .next_back()
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
    let mut file = File::create(&path).with_context(|| format!("'{}'", path.display()))?;
    file.write_all(&buf)?;
    while state.stage() != zmodem2::Stage::SessionEnd {
        if let Err(e) = zmodem2::receive(&mut port, &mut file, &mut state) {
            pb.finish_with_message("Aborted");
            bail!("ZMODEM error: {:#}", e);
        }
        pb.set_position(state.count() as u64);
    }
    pb.finish_with_message("Done");
    Ok(())
}
