// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2025 Jarkko Sakkinen

use nix::fcntl::{self, OFlag};
use nix::unistd;
use std::cmp::{max, min};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Result, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use zmodem2::{Poll, Stage, State};

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
    for (i, byte) in buffer.iter_mut().enumerate() {
        *byte = (i % 256) as u8;
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

/// Sets the O_NONBLOCK flag on a raw file descriptor.
fn set_nonblocking(fd: RawFd) {
    let flags = fcntl::fcntl(fd, fcntl::FcntlArg::F_GETFL).unwrap();
    let mut nonblocking_flags = OFlag::from_bits_truncate(flags);
    nonblocking_flags.insert(OFlag::O_NONBLOCK);
    fcntl::fcntl(fd, fcntl::FcntlArg::F_SETFL(nonblocking_flags)).unwrap();
}

/// Helper to set up a non-blocking `sz` process.
fn setup_sz(test_files: &TestFiles) -> (Child, MockPort<ChildStdout, ChildStdin>) {
    let mut sz_process = Command::new("sz")
        .args(&test_files.paths)
        .stdout(Stdio::piped())
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();

    let stdin = sz_process.stdin.take().unwrap();
    let stdout = sz_process.stdout.take().unwrap();

    set_nonblocking(stdin.as_raw_fd());
    set_nonblocking(stdout.as_raw_fd());

    let port = MockPort::new(stdout, stdin, RATE_BPS);
    (sz_process, port)
}

/// Helper to set up a non-blocking `rz` process.
fn setup_rz(dest_dir: &TempDir) -> (Child, MockPort<ChildStdout, ChildStdin>) {
    let mut rz_process: Child = Command::new("rz")
        .stdout(Stdio::piped())
        .stdin(Stdio::piped())
        .current_dir(dest_dir.path())
        .spawn()
        .unwrap();

    let stdin = rz_process.stdin.take().unwrap();
    let stdout = rz_process.stdout.take().unwrap();

    set_nonblocking(stdin.as_raw_fd());
    set_nonblocking(stdout.as_raw_fd());

    let port = MockPort::new(stdout, stdin, RATE_BPS);
    (rz_process, port)
}

#[test]
#[cfg(host_has_rzsz)]
fn test_batch_from_sz() {
    let test_files = TestFiles::new();
    let dest_dir = tempfile::Builder::new()
        .prefix("zmodem_test_dest_")
        .tempdir()
        .unwrap();

    let (mut sz_process, mut port) = setup_sz(&test_files);
    let mut state = State::new();
    let mut open_files: HashMap<Vec<u8>, File> = HashMap::new();
    let mut sink = std::io::sink();
    let mut current_file_name_bytes: Vec<u8> = Vec::new();

    while state.stage() != Stage::SessionEnd {
        if state.stage() == Stage::FileBegin
            && state.file_name() != current_file_name_bytes.as_slice()
        {
            let filename_bytes = state.file_name();
            let filename_str = std::str::from_utf8(filename_bytes).unwrap();
            let filename = Path::new(filename_str)
                .file_name()
                .unwrap()
                .to_str()
                .unwrap();
            let file_path = dest_dir.path().join(filename);
            let file = File::create(file_path).unwrap();
            open_files.insert(filename_bytes.to_vec(), file);
            current_file_name_bytes = filename_bytes.to_vec();
        }

        let mut file_writer: &mut dyn Write = open_files
            .get_mut(&current_file_name_bytes)
            .map(|f| f as &mut dyn Write)
            .unwrap_or(&mut sink);

        match state.receive(&mut port, &mut file_writer) {
            Ok(Poll::Ready) => continue,
            Ok(Poll::Pending) => {
                sleep(Duration::from_millis(10));
            }
            Err(e) => panic!("receive failed: {e}"),
        }
    }

    let mut drain = [0u8; 1024];
    while let Ok(Poll::Pending) = state.receive(&mut port, &mut sink) {
        let _ = unistd::read(port.r.as_raw_fd(), &mut drain);
        sleep(Duration::from_millis(10));
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
#[cfg(host_has_rzsz)]
fn test_batch_to_rz() {
    let test_files = TestFiles::new();
    let dest_dir = tempfile::Builder::new()
        .prefix("zmodem_test_dest_")
        .tempdir()
        .unwrap();

    let (mut rz_process, mut port) = setup_rz(&dest_dir);

    let mut open_files: HashMap<String, File> = HashMap::new();
    for path in &test_files.paths {
        let filename = path.file_name().unwrap().to_str().unwrap().to_string();
        let file = File::open(path).unwrap();
        open_files.insert(filename, file);
    }

    let mut file_iter = test_files.paths.iter();

    let first_path = file_iter.next().expect("No test files found");
    let first_filename = first_path.file_name().unwrap().to_str().unwrap();
    let first_size = first_path.metadata().unwrap().len() as u32;
    let mut state = State::set_first_file(first_filename, first_size).unwrap();

    'send_loop: while state.stage() != Stage::SessionEnd {
        if state.stage() == Stage::FileEnd {
            if let Some(next_path) = file_iter.next() {
                let next_filename = next_path.file_name().unwrap().to_str().unwrap();
                let next_size = next_path.metadata().unwrap().len() as u32;
                state.set_next_file(next_filename, next_size).unwrap();
            } else {
                break 'send_loop;
            }
        }

        let current_filename_bytes = state.file_name();
        let current_filename_str = std::str::from_utf8(current_filename_bytes).unwrap();
        let file = open_files
            .get_mut(current_filename_str)
            .expect("File not found in map");

        match state.send(&mut port, file) {
            Ok(Poll::Ready) => continue,
            Ok(Poll::Pending) => {
                sleep(Duration::from_millis(10));
            }
            Err(e) => panic!("send failed: {e}"),
        }
    }

    while state.stage() != Stage::SessionEnd {
        match state.finish(&mut port) {
            Ok(Poll::Ready) => {}
            Ok(Poll::Pending) => {
                sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("finish failed: {e}"),
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
