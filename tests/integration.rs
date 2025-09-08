// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2025 Jarkko Sakkinen

extern crate zmodem2;

use std::io::*;

#[allow(dead_code)]
struct InOut<R: Read, W: Write> {
    r: R,
    w: W,
}

#[allow(dead_code)]
impl<R: Read, W: Write> InOut<R, W> {
    pub fn new(r: R, w: W) -> InOut<R, W> {
        InOut { r, w }
    }
}

impl<R: Read, W: Write> Read for InOut<R, W> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.r.read(buf)
    }
}

impl<R: Read, W: Write> Write for InOut<R, W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.w.write(buf)
    }

    fn flush(&mut self) -> Result<()> {
        self.w.flush()
    }
}

#[allow(dead_code)]
const TEST_DATA: &[u8] = include_bytes!("test.bin");
#[allow(dead_code)]
const TMP_DIR: &str = env!("CARGO_TARGET_TMPDIR");

#[test]
#[cfg(host_has_rzsz)]
fn test_from_sz() {
    let file_name = format!("{TMP_DIR}/from_sz.bin");
    let mut file = std::fs::File::create(&file_name).unwrap();
    file.write_all(TEST_DATA).unwrap();
    let sz = std::process::Command::new("sz")
        .arg(&file_name)
        .stdout(std::process::Stdio::piped())
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = sz.stdin.unwrap();
    let stdout = sz.stdout.unwrap();
    let mut port = InOut::new(stdout, stdin);
    let mut file = Cursor::new(Vec::new());
    let mut state = zmodem2::State::new();
    while state.stage() != zmodem2::Stage::Done {
        assert!(zmodem2::receive(&mut port, &mut file, &mut state) == Ok(()));
    }
    assert_eq!(TEST_DATA, file.into_inner());
}

#[test]
#[cfg(host_has_rzsz)]
fn test_to_rz() {
    let file_name = format!("{TMP_DIR}/to_rz.bin");
    std::fs::remove_file(&file_name).unwrap_or_default();
    let sz = std::process::Command::new("rz")
        .stdout(std::process::Stdio::piped())
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = sz.stdin.unwrap();
    let stdout = sz.stdout.unwrap();
    let mut port = InOut::new(stdout, stdin);
    let len = TEST_DATA.len() as u32;
    let mut file = Cursor::new(TEST_DATA);
    let mut state = zmodem2::State::new_file(&file_name, len).unwrap();
    while state.stage() != zmodem2::Stage::Done {
        assert!(zmodem2::send(&mut port, &mut file, &mut state) == Ok(()));
    }
    let mut f = std::fs::File::open(&file_name).unwrap();
    let mut received = Vec::new();
    f.read_to_end(&mut received).unwrap();
    assert!(TEST_DATA == received);
}
