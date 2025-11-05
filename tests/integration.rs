// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2025 Jarkko Sakkinen

use std::cmp::{max, min};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Result, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use zmodem2::{finish, receive, send, Stage, State};

const FILE_COUNT: usize = 10;
const FILE_SIZE: usize = 50 * 1024;
const RATE_BPS: u32 = 115200;

const NAME_PREFIX: &[&str] = &[
    "Laser",
    "Neon",
    "Chrome",
    "Cosmic",
    "Turbo",
    "Starlight",
    "Future",
];
const NAME_POSTFIX: &[&str] = &[
    "Rider", "Funk", "Dream", "Grid", "System", "Dancer", "Midnight",
];
const EXTENSIONS: &[&str] = &["dat", "BIN", "log", "TMP", "txt"];

struct MockPort<R: Read, W: Write> {
    r: R,
    w: W,
    bits_per_second: u32,
    next_byte_due: Instant,
}

impl<R: Read, W: Write> MockPort<R, W> {
    pub fn new(r: R, w: W, bits_per_second: u32) -> Self {
        MockPort {
            r,
            w,
            bits_per_second,
            next_byte_due: Instant::now(),
        }
    }

    fn throttle(&mut self, bytes_transferred: usize) {
        if self.bits_per_second == 0 {
            return;
        }
        let bits_transferred = (bytes_transferred * 10) as f64;
        let duration_needed =
            Duration::from_secs_f64(bits_transferred / f64::from(self.bits_per_second));
        let now = Instant::now();
        if self.next_byte_due > now {
            sleep(self.next_byte_due - now);
        }
        self.next_byte_due = max(now, self.next_byte_due) + duration_needed;
    }
}

impl<R: Read, W: Write> Read for MockPort<R, W> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let bytes_read = self.r.read(buf)?;
        if bytes_read > 0 {
            self.throttle(bytes_read);
        }
        Ok(bytes_read)
    }
}

impl<R: Read, W: Write> Write for MockPort<R, W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let bytes_written = self.w.write(buf)?;
        if bytes_written > 0 {
            self.throttle(bytes_written);
        }
        Ok(bytes_written)
    }

    fn flush(&mut self) -> Result<()> {
        self.w.flush()
    }
}

/// Creates a temporary file with a predictable, patterned content.
fn create_test_file(path: &Path, size_bytes: usize) {
    let mut file = File::create(path).unwrap();
    let mut buffer = [0u8; 1024];
    for i in 0..buffer.len() {
        buffer[i] = (i % 256) as u8;
    }

    let mut bytes_written = 0;
    while bytes_written < size_bytes {
        let to_write = min(buffer.len(), size_bytes - bytes_written);
        file.write_all(&buffer[..to_write]).unwrap();
        bytes_written += to_write;
    }
}

/// Asserts that two files have the same size and content.
fn assert_files_equal(path1: &Path, path2: &Path) {
    let meta1 = path1.metadata().unwrap();
    let meta2 = path2.metadata().unwrap();
    assert_eq!(meta1.len(), meta2.len(), "File sizes do not match");

    let mut f1 = BufReader::new(File::open(path1).unwrap());
    let mut f2 = BufReader::new(File::open(path2).unwrap());

    loop {
        let buf1 = f1.fill_buf().unwrap();
        let buf2 = f2.fill_buf().unwrap();

        if buf1.is_empty() && buf2.is_empty() {
            break;
        }

        assert_eq!(buf1, buf2, "File contents do not match");

        let len1 = buf1.len();
        let len2 = buf2.len();
        f1.consume(len1);
        f2.consume(len2);
    }
}

/// Manages a set of temporary files for a test run.
struct TestFiles {
    #[allow(dead_code)]
    dir: TempDir,
    paths: Vec<PathBuf>,
}

impl TestFiles {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("zmodem_test_src_")
            .tempdir()
            .unwrap();
        let mut paths = Vec::new();
        for i in 0..FILE_COUNT {
            let prefix = NAME_PREFIX[i % NAME_PREFIX.len()];
            let postfix = NAME_POSTFIX[i % NAME_POSTFIX.len()];
            let ext = EXTENSIONS[i % EXTENSIONS.len()];
            let filename = format!("{prefix}{postfix}_{i}.{ext}");
            let path = dir.path().join(filename);

            create_test_file(&path, FILE_SIZE);
            paths.push(path);
        }
        Self { dir, paths }
    }
}

#[test]
#[cfg(all(host_has_rzsz))]
fn test_batch_from_sz() {
    let test_files = TestFiles::new();
    let dest_dir = tempfile::Builder::new()
        .prefix("zmodem_test_dest_")
        .tempdir()
        .unwrap();

    let mut sz_process = Command::new("sz")
        .args(&test_files.paths)
        .stdout(Stdio::piped())
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = sz_process.stdin.take().unwrap();
    let stdout = sz_process.stdout.take().unwrap();
    let mut port = MockPort::new(stdout, stdin, RATE_BPS);
    let mut state = State::new();
    let mut open_files: HashMap<String, File> = HashMap::new();
    let mut sink = std::io::sink();
    let mut current_file_name = String::new();
    while state.stage() != Stage::SessionEnd {
        if state.stage() == Stage::FileBegin && state.file_name() != current_file_name {
            let filename = Path::new(state.file_name())
                .file_name()
                .unwrap()
                .to_str()
                .unwrap();
            let file_path = dest_dir.path().join(filename);
            let file = File::create(file_path).unwrap();
            open_files.insert(filename.to_string(), file);
            current_file_name = state.file_name().to_string();
        }

        let current_filename = state.file_name().to_string();
        let mut file_writer: &mut dyn Write = open_files
            .get_mut(&current_filename)
            .map(|f| f as &mut dyn Write)
            .unwrap_or(&mut sink);

        assert!(receive(&mut port, &mut file_writer, &mut state).is_ok());
    }
    sz_process.wait().unwrap();
    for path in &test_files.paths {
        let filename = path.file_name().unwrap();
        let received_path = dest_dir.path().join(filename);
        assert!(
            received_path.exists(),
            "File '{}' was not received",
            received_path.display()
        );
        assert_files_equal(path, &received_path);
    }
}

#[test]
#[cfg(all(host_has_rzsz))]
fn test_batch_to_rz() {
    let test_files = TestFiles::new();
    let dest_dir = tempfile::Builder::new()
        .prefix("zmodem_test_dest_")
        .tempdir()
        .unwrap();

    let mut rz_process: Child = Command::new("rz")
        .stdout(Stdio::piped())
        .stdin(Stdio::piped())
        .current_dir(dest_dir.path())
        .spawn()
        .unwrap();
    let stdin = rz_process.stdin.take().unwrap();
    let stdout = rz_process.stdout.take().unwrap();
    let mut port = MockPort::new(stdout, stdin, RATE_BPS);
    let mut state = State::new();

    for path in &test_files.paths {
        let mut file = File::open(path).unwrap();
        let filename = path.file_name().unwrap().to_str().unwrap();
        let size = path.metadata().unwrap().len() as u32;

        state = State::new_file(filename, size).unwrap();
        while state.stage() != Stage::FileEnd {
            assert!(send(&mut port, &mut file, &mut state).is_ok());
        }
    }

    let mut attempts = 0usize;
    while state.stage() != Stage::SessionEnd && attempts < 8 {
        match finish(&mut port, &mut state) {
            Ok(()) => {}
            Err(zmodem2::Error::Write) | Err(zmodem2::Error::Read) => {
                break;
            }
            Err(e) => panic!("finish failed: {e}"),
        }
        attempts += 1;
        if state.stage() != Stage::SessionEnd {
            sleep(Duration::from_millis(50));
        }
    }

    rz_process.wait().unwrap();
    for path in &test_files.paths {
        let filename = path.file_name().unwrap();
        let received_path = dest_dir.path().join(filename);
        assert!(
            received_path.exists(),
            "File '{}' was not sent",
            received_path.display()
        );
        assert_files_equal(path, &received_path);
    }
}
