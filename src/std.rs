// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2025 Jarkko Sakkinen

use super::{Encoding, Error, Frame, Header, Packet, Read, Seek, String, Write};
use std::{fmt, io::SeekFrom};

impl<W> Write for W
where
    W: std::io::Write,
{
    fn write_all(&mut self, buf: &[u8]) -> Result<(), Error> {
        std::io::Write::write_all(self, buf).map_err(|e| {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                Error::WouldBlock
            } else {
                Error::Write(String::from(e.to_string().as_str()))
            }
        })
    }
}

impl<R> Read for R
where
    R: std::io::Read,
{
    fn read(&mut self, buf: &mut [u8]) -> Result<u32, Error> {
        let bytes_read = std::io::Read::read(self, buf).map_err(|e| {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                Error::WouldBlock
            } else {
                Error::Read(String::from(e.to_string().as_str()))
            }
        })?;
        u32::try_from(bytes_read).map_err(|_| Error::OutOfMemory)
    }

    fn read_byte(&mut self) -> Result<u8, Error> {
        let mut buf = [0; 1];
        match std::io::Read::read(self, &mut buf) {
            Ok(1) => Ok(buf[0]),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::WouldBlock {
                    Err(Error::WouldBlock)
                } else {
                    Err(Error::Read(String::from(e.to_string().as_str())))
                }
            }
            Ok(_) => Err(Error::NotConnected),
        }
    }
}

impl<S> Seek for S
where
    S: std::io::Seek,
{
    fn seek(&mut self, offset: u32) -> Result<u32, Error> {
        let new_offset = u64::from(offset);
        let final_offset = std::io::Seek::seek(self, SeekFrom::Start(new_offset)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                Error::WouldBlock
            } else {
                Error::Read(String::from(e.to_string().as_str()))
            }
        })?;
        u32::try_from(final_offset).map_err(|_| Error::UnexpectedEof)
    }
}

impl fmt::Display for Header {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:8} {}", self.encoding, self.frame)
    }
}

impl fmt::Display for Encoding {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:#02x}", *self as u8)
    }
}

impl fmt::Display for Frame {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:#02x}", *self as u8)
    }
}

impl fmt::Display for Packet {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:#0x}", *self as u8)
    }
}
