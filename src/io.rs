// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2023-2025 Jarkko Sakkinen

//! Traits for I/O non-blocking operations.

use crate::Error;

/// Write operations.
pub trait Write {
    /// Writes the entire buffer to the I/O port.
    ///
    /// # Errors
    ///
    /// [`Data`](crate::Error::Data) when corrupted data has been detected.
    /// [`Read`](crate::Error::Read) when the read I/O fails with the serial
    /// port.
    /// [`WouldBlock`](crate::Error::WouldBlock) when the I/O operation would
    /// block.
    /// [`Write`](crate::Error::Write) when the write I/O fails with the
    /// serial port.
    fn write_all(&mut self, buf: &[u8]) -> Result<(), Error>;

    /// Writes a single byte to the I/O port.
    ///
    /// # Errors
    ///
    /// [`Data`](crate::Error::Data) when corrupted data has been detected.
    /// [`Read`](crate::Error::Read) when the read I/O fails with the serial
    /// port.
    /// [`WouldBlock`](crate::Error::WouldBlock) when the I/O operation would
    /// block.
    /// [`Write`](crate::Error::Write) when the write I/O fails with the
    /// serial port.
    fn write_byte(&mut self, value: u8) -> Result<(), Error> {
        self.write_all(&[value])
    }
}

/// Read operations.
pub trait Read {
    /// Read bytes from the I/O port.
    ///
    /// # Errors
    ///
    /// * [`Data`](crate::Error::Data) when corrupted data has been detected.
    /// * [`Read`](crate::Error::Read) when the read I/O fails with the serial port.
    /// * [`WouldBlock`](crate::Error::WouldBlock) when the I/O operation would block.
    /// * [`Write`](crate::Error::Write) when the write I/O fails with the serial port.
    fn read(&mut self, buf: &mut [u8]) -> Result<u32, Error>;

    /// Read a byte from the I/O port.
    ///
    /// # Errors
    ///
    /// [`Data`](crate::Error::Data) when corrupted data has been detected.
    /// [`Read`](crate::Error::Read) when the read I/O fails with the serial port.
    /// [`WouldBlock`](crate::Error::WouldBlock) when the I/O operation would block.
    /// [`Write`](crate::Error::Write) when the write I/O fails with the serial port.
    fn read_byte(&mut self) -> Result<u8, Error>;
}

/// Seek operations
pub trait Seek {
    /// Seeks I/O port to an offset.
    ///
    /// # Errors
    ///
    /// [`Data`](crate::Error::Data) when corrupted data has been detected.
    /// [`Read`](crate::Error::Read) when the read I/O fails with the serial port.
    /// [`WouldBlock`](crate::Error::WouldBlock) when the I/O operation would block.
    /// [`Write`](crate::Error::Write) when the write I/O fails with the serial port.
    fn seek(&mut self, offset: u32) -> Result<u32, Error>;
}
