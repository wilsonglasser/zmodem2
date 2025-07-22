extern crate clap;
extern crate zmodem2;

mod stdinout;

use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::File;
use std::path::Path;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
#[command(about = "Receive a ZMODEM file transfer", long_about = None)]
pub struct Arguments {
    /// File path
    #[arg(short, long)]
    pub path: String,
}

fn main() {
    let args = Arguments::parse();
    let mut file = File::open(&args.path).unwrap();
    let filename = Path::new(&args.path).file_name().unwrap();
    let size = file.metadata().map(|x| x.len()).unwrap();
    let mut port = stdinout::CombinedStdInOut::new();
    let mut state = zmodem2::State::new_file(filename.to_str().unwrap(), size as u32).unwrap();

    let pb = ProgressBar::new(size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{msg}\n{spinner} [{elapsed_precise}] [{bar:40}] {bytes}/{total_bytes} ({eta})",
            )
            .unwrap()
            .progress_chars("=>-"),
    );
    pb.set_message(format!("Sending {}", filename.to_str().unwrap()));

    while state.stage() != zmodem2::Stage::Done {
        zmodem2::send(&mut port, &mut file, &mut state).unwrap();
        pb.set_position(state.count() as u64);
    }
    pb.finish_with_message("Done");
}
