// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2025 Jarkko Sakkinen

use std::io::{stdin, stdout, Read as StdRead, Stdin, Stdout, Write as StdWrite};

pub trait ReadWrite: zmodem2::Read + zmodem2::Write + StdRead + StdWrite {}

impl<T: zmodem2::Read + zmodem2::Write + StdRead + StdWrite> ReadWrite for T {}

pub struct CombinedStdInOut {
    stdin: Stdin,
    stdout: Stdout,
}

impl Default for CombinedStdInOut {
    fn default() -> Self {
        Self::new()
    }
}

impl CombinedStdInOut {
    pub fn new() -> CombinedStdInOut {
        CombinedStdInOut {
            stdin: stdin(),
            stdout: stdout(),
        }
    }
}

impl StdRead for CombinedStdInOut {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.stdin.read(buf)
    }
}

impl StdWrite for CombinedStdInOut {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stdout.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stdout.flush()
    }
}
