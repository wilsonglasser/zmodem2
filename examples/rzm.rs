extern crate clap;
extern crate zmodem2;

mod stdinout;

use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::File;
use std::io::Write;
use std::path::Path;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
#[command(about = "Receive a ZMODEM file transfer", long_about = None)]
pub struct Arguments {
    /// File path
    #[arg(short, long, default_value_t = String::default())]
    pub path: String,
}

fn main() {
    let mut port = stdinout::CombinedStdInOut::new();
    let mut state = zmodem2::State::new();
    let args = Arguments::parse();
    let mut buf = vec![];
    while state.stage() != zmodem2::Stage::InProgress {
        match zmodem2::receive(&mut port, &mut buf, &mut state) {
            Ok(()) => continue,
            _ => {
                eprintln!("RX error");
                return;
            }
        }
    }
    let path = if args.path.is_empty() {
        Path::new(state.file_name()).file_name().unwrap()
    } else {
        Path::new(&args.path).file_name().unwrap()
    };
    let pb = ProgressBar::new(state.file_size() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{msg}\n{spinner} [{elapsed_precise}] [{bar:40}] {bytes}/{total_bytes} ({eta})",
            )
            .unwrap()
            .progress_chars("=>-"),
    );
    pb.set_message(format!("Receiving {}", path.to_str().unwrap()));
    let mut file = File::create(path).unwrap();
    file.write_all(&buf).unwrap();
    while state.stage() != zmodem2::Stage::Done {
        match zmodem2::receive(&mut port, &mut file, &mut state) {
            Ok(()) => {
                pb.set_position(state.count() as u64);
                continue;
            }
            _ => {
                pb.finish_with_message("RX error");
                return;
            }
        }
    }
    pb.finish_with_message("Done");
}
