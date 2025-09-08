// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2025 Jarkko Sakkinen

use std::io::{stdin, stdout, Read, Result, Stdin, Stdout, Write};

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

impl Read for CombinedStdInOut {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.stdin.read(buf)
    }
}

impl Write for CombinedStdInOut {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let r = self.stdout.write(buf)?;
        Ok(r)
    }

    fn flush(&mut self) -> Result<()> {
        self.stdout.flush()
    }
}
