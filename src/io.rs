// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2023-2025 Jarkko Sakkinen

//! Traits and types for I/O operations.

use crate::Error;

/// Write I/O operations
pub trait Write {
    /// Attempts to write the entire buffer
    ///
    /// # Errors
    ///
    /// * `Err(Error::Read)` when the read I/O fails with the serial port
    /// * `Err(Error::Write)` when the write I/O fails with the serial port
    /// * `Err(Error::Data)` when corrupted data has been detected
    /// * `Err(Error::WouldBlock)` when the I/O operation would block
    fn write_all(&mut self, buf: &[u8]) -> Result<(), Error>;

    /// Attempts to write a single byte
    ///
    /// # Errors
    ///
    /// * `Err(Error::Read)` when the read I/O fails with the serial port
    /// * `Err(Error::Write)` when the write I/O fails with the serial port
    /// * `Err(Error::Data)` when corrupted data has been detected
    /// * `Err(Error::WouldBlock)` when the I/O operation would block
    fn write_byte(&mut self, value: u8) -> Result<(), Error> {
        self.write_all(&[value])
    }
}

/// Read I/O operations
pub trait Read {
    /// Reads some bytes to the buffeer
    ///
    /// # Errors
    ///
    /// * `Err(Error::Read)` when the read I/O fails with the serial port
    /// * `Err(Error::Write)` when the write I/O fails with the serial port
    /// * `Err(Error::Data)` when corrupted data has been detected
    /// * `Err(Error::WouldBlock)` when the I/O operation would block
    fn read(&mut self, buf: &mut [u8]) -> Result<u32, Error>;

    /// Reads exactly one byte to the buffer
    ///
    /// # Errors
    ///
    /// * `Err(Error::Read)` when the read I/O fails with the serial port
    /// * `Err(Error::Write)` when the write I/O fails with the serial port
    /// * `Err(Error::Data)` when corrupted data has been detected
    /// * `Err(Error::WouldBlock)` when the I/O operation would block
    fn read_byte(&mut self) -> Result<u8, Error>;
}

/// Seek I/O operations
pub trait Seek {
    /// Seeks to an offset
    ///
    /// # Errors
    ///
    /// * `Err(Error::Read)` when the read I/O fails with the serial port
    /// * `Err(Error::Write)` when the write I/O fails with the serial port
    /// * `Err(Error::Data)` when corrupted data has been detected
    /// * `Err(Error::WouldBlock)` when the I/O operation would block
    fn seek(&mut self, offset: u32) -> Result<u32, Error>;
}
