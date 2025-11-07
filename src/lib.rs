// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2025 Jarkko Sakkinen

//! ZMODEM file transfer protocol crate. `zmodem2::State::receive` and `zmodem2::State::send`
//! provide a synchronous and sequential API for sending and receiving files
//! with the ZMODEM protocol. Each step corresponds to a single ZMODEM frame
//! transaction, and the state between the calls is kept in a `zmodem2::State`
//! instance.
//!
//! The usage can be described in the high-level with the following flow:
//!
//! 1. Create `zmodem2::State`.
//! 2. Call either `state.send(...)` or `state.receive(...)`.
//! 3. If the returned `zmodem2::Stage` is not yet `zmodem2::Stage::FileEnd`,
//!    go back to step 2.

#![deny(clippy::all)]
#![deny(clippy::pedantic)]
#![allow(clippy::result_large_err)]
#![cfg_attr(not(feature = "std"), no_std)]

mod buffer;
mod crc;
mod error;
mod io;
#[cfg(feature = "std")]
mod std;
mod zdle;

pub use buffer::*;
pub use error::*;
pub use io::*;

use bitflags::bitflags;
use core::{
    cmp::min,
    convert::TryFrom,
    fmt,
    ops::{Deref, DerefMut},
};
use strum::EnumIter;
use strum::IntoEnumIterator;

/// Size of the unescaped subpacket payload. The size is picked from the
/// original ZMODEM specification.
const SUBPACKET_MAX_SIZE: usize = 1024;

/// Buffer size with enough capacity for an escaped header
const HEADER_SIZE: usize = 32;

/// The number of subpackets to stream
const SUBPACKET_PER_ACK: usize = 10;

/// Maximum number of bytes to represent a u32 as ASCII.
const U32_ASCII_MAX: usize = 10;

/// The capacity of the fixed-size `String` type.
const STRING_CAP: usize = 256;

pub const ZPAD: u8 = b'*';
pub const ZDLE: u8 = 0x18;
pub const XON: u8 = 0x11;

const ZACK_HEADER: Header = Header::new(Encoding::ZHEX, Frame::ZACK, &[0; 4]);
const ZDATA_HEADER: Header = Header::new(Encoding::ZBIN32, Frame::ZDATA, &[0; 4]);
const ZEOF_HEADER: Header = Header::new(Encoding::ZBIN32, Frame::ZEOF, &[0; 4]);
const ZFIN_HEADER: Header = Header::new(Encoding::ZHEX, Frame::ZFIN, &[0; 4]);
const ZNAK_HEADER: Header = Header::new(Encoding::ZHEX, Frame::ZNAK, &[0; 4]);
const ZRPOS_HEADER: Header = Header::new(Encoding::ZHEX, Frame::ZRPOS, &[0; 4]);
const ZRQINIT_HEADER: Header = Header::new(Encoding::ZHEX, Frame::ZRQINIT, &[0; 4]);

/// A stack-allocated, fixed-capacity string.
///
/// This is a newtype wrapper around `Buffer<256>` to provide type safety and
/// string-specific operations.
#[derive(Eq)]
pub struct String(Buffer<STRING_CAP>);

impl Default for String {
    fn default() -> Self {
        Self::new()
    }
}

impl From<&str> for String {
    /// Creates a new `String` from a `&str`, truncating if necessary.
    fn from(s: &str) -> Self {
        let mut string = Self::new();
        let bytes = s.as_bytes();
        let len = min(bytes.len(), STRING_CAP);
        let mut end = len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }

        let truncated_bytes = &bytes[..end];
        string
            .extend_from_slice(truncated_bytes)
            .unwrap_or_default();
        string
    }
}

impl String {
    /// Creates a new, empty string.
    #[must_use]
    pub const fn new() -> Self {
        Self(Buffer::<STRING_CAP>::new())
    }

    /// Resets buffer length back to zero.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Returns the capacity of the buffer in bytes.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.0.capacity()
    }

    /// Copies bytes from a slice to the end of the buffer.
    ///
    /// # Errors
    ///
    /// Returns `Err(CapacityError)`, if the capacity would be exceeded.
    pub fn extend_from_slice(&mut self, slice: &[u8]) -> Result<(), CapacityError> {
        self.0.extend_from_slice(slice)
    }
}

impl Deref for String {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for String {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl AsRef<[u8]> for String {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl<T: ?Sized> PartialEq<T> for String
where
    T: AsRef<[u8]>,
{
    fn eq(&self, other: &T) -> bool {
        self.as_ref() == other.as_ref()
    }
}

impl fmt::Debug for String {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match core::str::from_utf8(&self.0) {
            Ok(s) => f.write_str(s),
            Err(_) => f.debug_list().entries(self.0.iter()).finish(),
        }
    }
}

impl fmt::Display for String {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(core::str::from_utf8(&self.0).unwrap_or(""))
    }
}

/// Data structure for holding a ZMODEM protocol header, which begins a frame,
/// and is followed optionally by a variable number of subpackets.
#[repr(C)]
#[derive(PartialEq)]
pub struct Header {
    encoding: Encoding,
    frame: Frame,
    flags: [u8; 4],
}

impl Header {
    /// Creates a new instance
    #[must_use]
    pub const fn new(encoding: Encoding, frame: Frame, flags: &[u8; 4]) -> Self {
        Self {
            encoding,
            frame,
            flags: *flags,
        }
    }

    /// Returns `Encoding` of the frame
    #[must_use]
    pub const fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// Returns `Frame`, containing the frame type
    #[must_use]
    pub const fn frame(&self) -> Frame {
        self.frame
    }

    /// Returns count for the frame types using this field
    #[must_use]
    pub const fn count(&self) -> u32 {
        u32::from_le_bytes(self.flags)
    }

    /// Encodes and writes the header to the serial port
    ///
    /// # Errors
    ///
    /// * [`Read`](crate::Error::Read) when the read I/O fails with the serial port
    /// * [`Write`](crate::Error::Write) when the write I/O fails with the serial port
    /// * [`Data`](crate::Error::Data) when corrupted data has been detected
    pub fn write<P>(&self, port: &mut P) -> Result<Option<()>, Error>
    where
        P: Write + ?Sized,
    {
        let mut out: Buffer<HEADER_SIZE> = Buffer::new();

        if port.write_byte(ZPAD)?.is_none() {
            return Ok(None);
        }
        if self.encoding == Encoding::ZHEX && port.write_byte(ZPAD)?.is_none() {
            return Ok(None);
        }
        if port.write_byte(ZDLE)?.is_none() {
            return Ok(None);
        }
        if port.write_byte(self.encoding as u8)?.is_none() {
            return Ok(None);
        }

        out.push(self.frame as u8).map_err(|_| Error::OutOfMemory)?;
        out.extend_from_slice(&self.flags)
            .map_err(|_| Error::OutOfMemory)?;
        let mut crc = [0u8; 4];
        let crc_len = make_crc(&out, &mut crc, self.encoding);
        out.extend_from_slice(&crc[..crc_len])
            .map_err(|_| Error::OutOfMemory)?;

        if self.encoding == Encoding::ZHEX {
            let mut hex_buf = [0u8; HEADER_SIZE];
            let len = out.len() * 2;
            let hex = &mut hex_buf.get_mut(..len).ok_or(Error::UnexpectedEof)?;
            hex::encode_to_slice(&out, hex).map_err(|_| Error::OutOfMemory)?;
            if write_slice_escaped(port, hex)?.is_none() {
                return Ok(None);
            }
        } else if write_slice_escaped(port, &out)?.is_none() {
            return Ok(None);
        }

        if self.encoding == Encoding::ZHEX {
            if port.write_byte(b'\r')?.is_none() {
                return Ok(None);
            }
            if port.write_byte(b'\n')?.is_none() {
                return Ok(None);
            }
            if self.frame != Frame::ZACK
                && self.frame != Frame::ZFIN
                && port.write_byte(XON)?.is_none()
            {
                return Ok(None);
            }
        }
        Ok(Some(()))
    }

    /// Reads and decodes a header from the serial port, and returns a new
    /// instance
    ///
    /// # Errors
    ///
    /// * [`Read`](crate::Error::Read) when the read I/O fails with the serial port
    /// * [`Write`](crate::Error::Write) when the write I/O fails with the serial port
    /// * [`Data`](crate::Error::Data) when corrupted data has been detected
    pub fn read<P>(port: &mut P) -> Result<Option<Header>, Error>
    where
        P: Read + ?Sized,
    {
        let Some(encoding_byte) = port.read_byte()? else {
            return Ok(None);
        };
        let encoding = Encoding::try_from(encoding_byte)?;

        let mut out_hex: Buffer<HEADER_SIZE> = Buffer::new();
        for _ in 0..Header::unescaped_size(encoding) - 1 {
            let Some(byte) = read_byte_unescaped(port)? else {
                return Ok(None);
            };
            out_hex.push(byte).map_err(|_| Error::OutOfMemory)?;
        }

        let mut out: Buffer<HEADER_SIZE> = Buffer::new();
        if encoding == Encoding::ZHEX {
            let mut out_bytes = [0u8; HEADER_SIZE / 2];
            let out_len = out_hex.len() / 2;
            hex::decode_to_slice(&out_hex, &mut out_bytes[..out_len])
                .map_err(|_| Error::MalformedHeader)?;
            out.extend_from_slice(&out_bytes[..out_len])
                .map_err(|_| Error::OutOfMemory)?;
        } else {
            out.extend_from_slice(&out_hex)
                .map_err(|_| Error::OutOfMemory)?;
        }
        check_crc(&out[..5], &out[5..], encoding)?;
        let frame = Frame::try_from(out[0])?;
        let mut header = Header::new(encoding, frame, &[0; 4]);
        header.flags.copy_from_slice(&out[1..=4]);
        Ok(Some(header))
    }

    /// Returns a new instance with the flags substitude with a count
    /// for the frame types using this field.
    #[must_use]
    pub const fn with_count(&self, count: u32) -> Self {
        Header::new(self.encoding, self.frame, &count.to_le_bytes())
    }

    /// Returns the serialized size of the header before escaping
    const fn unescaped_size(encoding: Encoding) -> usize {
        match encoding {
            Encoding::ZBIN => core::mem::size_of::<Header>() + 2,
            Encoding::ZBIN32 => core::mem::size_of::<Header>() + 4,
            Encoding::ZHEX => (core::mem::size_of::<Header>() + 2) * 2 - 1,
        }
    }
}

/// The ZMODEM protocol frame encoding
#[repr(u8)]
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, EnumIter, PartialEq)]
pub enum Encoding {
    ZBIN = 0x41,
    ZHEX = 0x42,
    ZBIN32 = 0x43,
}

impl TryFrom<u8> for Encoding {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Encoding::iter()
            .find(|e| value == *e as u8)
            .ok_or(Error::MalformedEncoding(value))
    }
}

#[repr(u8)]
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, EnumIter, PartialEq)]
/// Frame types
pub enum Frame {
    /// Request receive init
    ZRQINIT = 0,
    /// Receiver capabilities and packet size
    ZRINIT = 1,
    /// Send init sequence (optional)
    ZSINIT = 2,
    /// ACK to above
    ZACK = 3,
    /// File name from sender
    ZFILE = 4,
    /// To sender: skip this file
    ZSKIP = 5,
    /// Last packet was garbled
    ZNAK = 6,
    /// Abort batch transfers
    ZABORT = 7,
    /// Finish session
    ZFIN = 8,
    /// Resume data trans at this position
    ZRPOS = 9,
    /// Data packet(s) follow
    ZDATA = 10,
    /// End of file
    ZEOF = 11,
    /// Fatal Read or Write error Detected
    ZFERR = 12,
    /// Request for file CRC and response
    ZCRC = 13,
    /// Receiver's Challenge
    ZCHALLENGE = 14,
    /// Request is complete
    ZCOMPL = 15,
    /// Other end canned session with CAN*5
    ZCAN = 16,
    /// Request for free bytes on filesystem
    ZFREECNT = 17,
    /// Command from sending program
    ZCOMMAND = 18,
    /// Output to standard error, data follows
    ZSTDERR = 19,
}

impl TryFrom<u8> for Frame {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Frame::iter()
            .find(|t| value == *t as u8)
            .ok_or(Error::MalformedFrame(value))
    }
}

bitflags! {
    /// `ZRINIT` flags
    struct Zrinit: u8 {
        /// Can send and receive in full-duplex
        const CANFDX = 0x01;
        /// Can receive data in parallel with disk I/O
        const CANOVIO = 0x02;
        /// Can send a break signal
        const CANBRK = 0x04;
        /// Can decrypt
        const CANCRY = 0x08;
        /// Can uncompress
        const CANLZW = 0x10;
        /// Can use 32-bit frame check
        const CANFC32 = 0x20;
        /// Expects control character to be escaped
        const ESCCTL = 0x40;
        /// Expects 8th bit to be escaped
        const ESC8 = 0x80;
    }
}

/// The ZMODEM protocol subpacket type
#[repr(u8)]
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, EnumIter, PartialEq)]
pub enum Packet {
    ZCRCE = 0x68,
    ZCRCG = 0x69,
    ZCRCQ = 0x6a,
    ZCRCW = 0x6b,
}

impl TryFrom<u8> for Packet {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Packet::iter()
            .find(|e| value == *e as u8)
            .ok_or(Error::MalformedPacket(value))
    }
}

/// Send or receive transmission state
pub struct State {
    stage: Stage,
    count: u32,
    file_name: String,
    file_size: u32,
    buf: Buffer<SUBPACKET_MAX_SIZE>,
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl State {
    /// Create a new transmission context
    #[must_use]
    pub const fn new() -> Self {
        Self {
            stage: Stage::SessionBegin,
            count: 0,
            file_name: String::new(),
            file_size: 0,
            buf: Buffer::<SUBPACKET_MAX_SIZE>::new(),
        }
    }

    /// Create a new transmission context for the first file in a batch.
    ///
    /// # Errors
    ///
    /// * [`Read`](crate::Error::Read) when the read I/O fails with the serial port
    /// * [`Write`](crate::Error::Write) when the write I/O fails with the serial port
    /// * [`Data`](crate::Error::Data) when corrupted data has been detected
    pub fn set_first_file(file_name: &str, file_size: u32) -> Result<Self, Error> {
        let mut state = Self::new();
        state
            .file_name
            .extend_from_slice(file_name.as_bytes())
            .map_err(|_| Error::OutOfMemory)?;
        state.file_size = file_size;
        Ok(state)
    }

    /// Prepares the state for the next file in a batch transfer.
    ///
    /// # Errors
    ///
    /// * [`Data`](crate::Error::Data) when the `file_name` is invalid.
    pub fn set_next_file(&mut self, file_name: &str, file_size: u32) -> Result<(), Error> {
        self.file_name.clear();
        self.file_name
            .extend_from_slice(file_name.as_bytes())
            .map_err(|_| Error::OutOfMemory)?;
        self.file_size = file_size;
        self.count = 0;
        self.stage = Stage::FileEnd;
        Ok(())
    }

    #[must_use]
    pub fn stage(&self) -> Stage {
        self.stage
    }

    #[must_use]
    pub fn count(&self) -> u32 {
        self.count
    }

    #[must_use]
    pub fn file_name(&self) -> &[u8] {
        &self.file_name
    }

    #[must_use]
    pub fn file_size(&self) -> u32 {
        self.file_size
    }

    /// Sends a file using the ZMODEM file transfer protocol.
    ///
    /// # Errors
    ///
    /// * [`Read`](crate::Error::Read) when the read I/O fails with the serial port
    /// * [`Write`](crate::Error::Write) when the write I/O fails with the serial port
    /// * [`Data`](crate::Error::Data) when corrupted data has been detected
    pub fn send<P, F>(&mut self, port: &mut P, file: &mut F) -> Result<Option<()>, Error>
    where
        P: Read + Write + ?Sized,
        F: Read + Seek + ?Sized,
    {
        self.send_impl(port, file)
    }

    fn send_impl<P, F>(&mut self, port: &mut P, file: &mut F) -> Result<Option<()>, Error>
    where
        P: Read + Write + ?Sized,
        F: Read + Seek + ?Sized,
    {
        if self.stage == Stage::SessionBegin {
            if ZRQINIT_HEADER.write(port)?.is_none() {
                return Ok(None);
            }
        } else if self.stage == Stage::FileEnd {
            if write_zfile(port, &mut self.buf, &self.file_name, self.file_size)?.is_none() {
                return Ok(None);
            }
            self.stage = Stage::FileBegin;
            return Ok(Some(()));
        }

        let Some(()) = read_zpad(port)? else {
            return Ok(None);
        };

        let header = match Header::read(port) {
            Ok(Some(header)) => header,
            Ok(None) => return Ok(None),
            Err(e) => {
                if ZNAK_HEADER.write(port)?.is_none() {
                    return Ok(None);
                }
                return Err(e);
            }
        };

        match header.frame() {
            Frame::ZRINIT => {
                if self.stage == Stage::SessionBegin {
                    if write_zfile(port, &mut self.buf, &self.file_name, self.file_size)?.is_none()
                    {
                        return Ok(None);
                    }
                    self.stage = Stage::FileBegin;
                } else if self.stage == Stage::FileInProgress {
                    self.stage = Stage::FileEnd;
                }
            }
            Frame::ZRPOS | Frame::ZACK => {
                if self.stage == Stage::SessionBegin {
                    if ZRQINIT_HEADER.write(port)?.is_none() {
                        return Ok(None);
                    }
                } else if self.stage == Stage::FileBegin || self.stage == Stage::FileInProgress {
                    if write_zdata(port, file, header.count())?.is_none() {
                        return Ok(None);
                    }
                    self.stage = Stage::FileInProgress;
                }
            }
            _ => {
                if self.stage == Stage::SessionBegin && ZRQINIT_HEADER.write(port)?.is_none() {
                    return Ok(None);
                }
            }
        }
        Ok(Some(()))
    }

    /// Receives a file using the ZMODEM file transfer protocol.
    ///
    /// # Errors
    ///
    /// * [`Read`](crate::Error::Read) when the read I/O fails with the serial port
    /// * [`Write`](crate::Error::Write) when the write I/O fails with the serial port
    /// * [`Data`](crate::Error::Data) when corrupted data has been detected
    pub fn receive<P, F>(&mut self, port: &mut P, file: &mut F) -> Result<Option<()>, Error>
    where
        P: Read + Write + ?Sized,
        F: Write + ?Sized,
    {
        self.receive_impl(port, file)
    }

    fn receive_impl<P, F>(&mut self, port: &mut P, file: &mut F) -> Result<Option<()>, Error>
    where
        P: Read + Write + ?Sized,
        F: Write + ?Sized,
    {
        if self.stage == Stage::SessionBegin && write_zrinit(port)?.is_none() {
            return Ok(None);
        }

        let Some(()) = (match read_zpad(port) {
            Ok(Some(v)) => Ok(Some(v)),
            Ok(None) => return Ok(None),
            Err(Error::Read(err)) => {
                if self.stage == Stage::FileBegin {
                    self.stage = Stage::SessionEnd;
                    return Ok(Some(()));
                }
                Err(Error::Read(err))
            }
            Err(e) => Err(e),
        })?
        else {
            return Ok(None);
        };

        let header = match Header::read(port) {
            Ok(Some(header)) => header,
            Ok(None) => return Ok(None),
            Err(Error::Read(err)) => {
                if self.stage == Stage::FileBegin {
                    self.stage = Stage::SessionEnd;
                    return Ok(Some(()));
                }
                return Err(Error::Read(err));
            }
            Err(e) => {
                if ZNAK_HEADER.write(port)?.is_none() {
                    return Ok(None);
                }
                return Err(e);
            }
        };

        match header.frame() {
            Frame::ZFILE => {
                if self.stage == Stage::SessionBegin || self.stage == Stage::FileBegin {
                    if read_zfile(port, self, header.encoding())?.is_none() {
                        return Ok(None);
                    }
                    self.stage = Stage::FileBegin;
                }
            }
            Frame::ZDATA => {
                if self.stage == Stage::SessionBegin {
                    if write_zrinit(port)?.is_none() {
                        return Ok(None);
                    }
                } else if self.stage == Stage::FileBegin || self.stage == Stage::FileInProgress {
                    if header.count() != self.count {
                        if ZRPOS_HEADER.with_count(self.count).write(port)?.is_none() {
                            return Ok(None);
                        }
                        return Ok(Some(()));
                    }
                    if read_zdata(port, self, header.encoding(), file)?.is_none() {
                        return Ok(None);
                    }
                    self.stage = Stage::FileInProgress;
                }
            }
            Frame::ZEOF => {
                if self.stage == Stage::FileInProgress && header.count() == self.count {
                    if write_zrinit(port)?.is_none() {
                        return Ok(None);
                    }
                    self.stage = Stage::FileBegin;
                }
            }
            Frame::ZFIN => {
                if self.stage == Stage::FileInProgress || self.stage == Stage::FileBegin {
                    if ZFIN_HEADER.write(port)?.is_none() {
                        return Ok(None);
                    }
                    self.stage = Stage::SessionEnd;
                }
            }
            _ => {}
        }
        Ok(Some(()))
    }

    /// Send ZFIN.
    ///
    /// # Errors
    ///
    /// * [`Read`](crate::Error::Read) when the read I/O fails with the serial port
    /// * [`Write`](crate::Error::Write) when the write I/O fails with the serial port
    /// * [`Data`](crate::Error::Data) when corrupted data has been detected
    pub fn finish<P>(&mut self, port: &mut P) -> Result<Option<()>, Error>
    where
        P: Read + Write + ?Sized,
    {
        self.finish_impl(port)
    }

    fn finish_impl<P>(&mut self, port: &mut P) -> Result<Option<()>, Error>
    where
        P: Read + Write + ?Sized,
    {
        if ZFIN_HEADER.write(port)?.is_none() {
            return Ok(None);
        }

        let Some(()) = read_zpad(port)? else {
            return Ok(None);
        };

        let frame = match Header::read(port) {
            Ok(Some(frame)) => frame,
            Ok(None) => return Ok(None),
            Err(e) => {
                if ZNAK_HEADER.write(port)?.is_none() {
                    return Ok(None);
                }
                return Err(e);
            }
        };

        match frame.frame() {
            Frame::ZFIN | Frame::ZRINIT => {
                if port.write_byte(b'O')?.is_none() {
                    return Ok(None);
                }
                if port.write_byte(b'O')?.is_none() {
                    return Ok(None);
                }
                self.stage = Stage::SessionEnd;
            }
            _ => {
                if ZFIN_HEADER.write(port)?.is_none() {
                    return Ok(None);
                }
            }
        }

        Ok(Some(()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stage {
    FileBegin,
    FileEnd,
    FileInProgress,
    SessionBegin,
    SessionEnd,
}

/// Writes ZRINIT
fn write_zrinit<P>(port: &mut P) -> Result<Option<()>, Error>
where
    P: Write + ?Sized,
{
    let zrinit = Zrinit::CANFDX | Zrinit::CANOVIO | Zrinit::CANFC32;
    Header::new(Encoding::ZHEX, Frame::ZRINIT, &[0, 0, 0, zrinit.bits()]).write(port)
}

/// Converts a u32 to its ASCII byte representation.
///
/// This function writes the ASCII digits of `value` into `buf` from right to
/// left and returns a slice pointing to the written digits.
///
/// Example: `value = 123`, `buf = [0u8; 10]`
/// -> `buf` becomes `[..., 0, 0, 0, 0, 1, 2, 3]`
/// -> returns `&[1, 2, 3]`
fn u32_to_ascii_bytes(mut value: u32, buf: &mut [u8; U32_ASCII_MAX]) -> &[u8] {
    if value == 0 {
        buf[U32_ASCII_MAX - 1] = b'0';
        return &buf[U32_ASCII_MAX - 1..];
    }

    let mut index = U32_ASCII_MAX;
    while value > 0 {
        index -= 1;
        buf[index] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    &buf[index..]
}

/// Parses a u32 from a slice of ASCII decimal bytes.
fn parse_file_size(bytes: &[u8]) -> Result<u32, Error> {
    if bytes.is_empty() {
        return Ok(0);
    }

    let mut result: u32 = 0;
    for &byte in bytes {
        let digit = match byte {
            b'0'..=b'9' => u32::from(byte - b'0'),
            _ => return Err(Error::MalformedFileSize),
        };
        result = result
            .checked_mul(10)
            .and_then(|r| r.checked_add(digit))
            .ok_or(Error::MalformedFileSize)?;
    }
    Ok(result)
}

/// Write ZRFILE
fn write_zfile<P>(
    port: &mut P,
    buf: &mut Buffer<SUBPACKET_MAX_SIZE>,
    name: &[u8],
    size: u32,
) -> Result<Option<()>, Error>
where
    P: Write + ?Sized,
{
    let mut size_buf = [0u8; U32_ASCII_MAX];
    let size_bytes = u32_to_ascii_bytes(size, &mut size_buf);

    buf.clear();
    buf.extend_from_slice(name)
        .map_err(|_| Error::OutOfMemory)?;
    buf.push(b'\0').map_err(|_| Error::OutOfMemory)?;
    buf.extend_from_slice(size_bytes)
        .map_err(|_| Error::OutOfMemory)?;
    buf.push(b'\0').map_err(|_| Error::OutOfMemory)?;

    if Header::new(Encoding::ZBIN32, Frame::ZFILE, &[0; 4])
        .write(port)?
        .is_none()
    {
        return Ok(None);
    }
    write_subpacket(port, Encoding::ZBIN32, Packet::ZCRCW, buf)
}

/// Parses filename and size from the subpacket sent after the `Frame::ZFiLE`
/// header.
fn read_zfile<P>(port: &mut P, state: &mut State, encoding: Encoding) -> Result<Option<()>, Error>
where
    P: Read + Write + ?Sized,
{
    match read_subpacket(port, &mut state.buf, encoding) {
        Ok(Some(_)) => {
            let payload = &state.buf;
            let mut fields = payload.split(|&b| b == b'\0');

            let file_name_bytes = fields.next().ok_or(Error::MalformedFileName)?;
            if file_name_bytes.is_empty() {
                return Err(Error::MalformedFileName);
            }

            core::str::from_utf8(file_name_bytes).map_err(|_| Error::MalformedFileName)?;

            state.file_name.clear();
            state
                .file_name
                .extend_from_slice(file_name_bytes)
                .map_err(|_| Error::OutOfMemory)?;

            if let Some(size_str_bytes) = fields.next() {
                let size_field_bytes = size_str_bytes
                    .split(|&b| b == b' ')
                    .next()
                    .unwrap_or_default();

                state.file_size = parse_file_size(size_field_bytes)?;
            } else {
                state.file_size = 0;
            }

            state.count = 0;

            ZRPOS_HEADER.with_count(0).write(port)
        }
        Ok(None) => Ok(None),
        Err(e) => {
            if ZNAK_HEADER.write(port)?.is_none() {
                return Ok(None);
            }
            Err(e)
        }
    }
}

/// Writes ZDATA
fn write_zdata<P, F>(port: &mut P, file: &mut F, offset: u32) -> Result<Option<()>, Error>
where
    P: Read + Write + ?Sized,
    F: Read + Seek + ?Sized,
{
    let mut offset = offset;
    let mut local_buf = [0u8; SUBPACKET_MAX_SIZE];
    let read_buf = &mut local_buf[..SUBPACKET_MAX_SIZE];

    let Some(new_offset) = file.seek(offset)? else {
        return Ok(None);
    };
    if new_offset != offset {
        return Err(Error::UnexpectedEof);
    }

    let Some(count) = file.read(read_buf)? else {
        return Ok(None);
    };
    let mut count = count as usize;

    if count == 0 {
        ZEOF_HEADER.with_count(offset).write(port)?;
        return Ok(Some(()));
    }
    if ZDATA_HEADER.with_count(offset).write(port)?.is_none() {
        return Ok(None);
    }
    for _ in 1..SUBPACKET_PER_ACK {
        if write_subpacket(port, Encoding::ZBIN32, Packet::ZCRCG, &read_buf[..count])?.is_none() {
            return Ok(None);
        }
        offset += u32::try_from(count).map_err(|_| Error::OutOfMemory)?;

        let Some(new_count) = file.read(read_buf)? else {
            return Ok(None);
        };
        count = new_count as usize;
        if count < read_buf.len() {
            break;
        }
    }
    write_subpacket(port, Encoding::ZBIN32, Packet::ZCRCW, &read_buf[..count])
}

/// Reads ZDATA
fn read_zdata<P, F>(
    port: &mut P,
    state: &mut State,
    encoding: Encoding,
    file: &mut F,
) -> Result<Option<()>, Error>
where
    P: Read + Write + ?Sized,
    F: Write + ?Sized,
{
    loop {
        let zcrc = match read_subpacket(port, &mut state.buf, encoding) {
            Ok(Some(zcrc)) => zcrc,
            Ok(None) => return Ok(None),
            Err(e) => {
                if ZNAK_HEADER.with_count(state.count).write(port)?.is_none() {
                    return Ok(None);
                }
                return Err(e);
            }
        };

        if file.write_all(&state.buf)?.is_none() {
            return Ok(None);
        }
        state.count += u32::try_from(state.buf.len()).map_err(|_| Error::OutOfMemory)?;
        match zcrc {
            Packet::ZCRCW => {
                if ZACK_HEADER.with_count(state.count).write(port)?.is_none() {
                    return Ok(None);
                }
                return Ok(Some(()));
            }
            Packet::ZCRCE => {
                return Ok(Some(()));
            }
            Packet::ZCRCQ => {
                if ZACK_HEADER.with_count(state.count).write(port)?.is_none() {
                    return Ok(None);
                }
            }
            Packet::ZCRCG => (),
        }
    }
}

/// Skips (ZPAD, [ZPAD,] ZDLE) sequence.
///
/// # Errors
///
/// Returns `Error::Data` if the sequence is malformed or `Error::Read` on an
/// I/O error.
fn read_zpad<P>(port: &mut P) -> Result<Option<()>, Error>
where
    P: Read + ?Sized,
{
    loop {
        let Some(b) = port.read_byte()? else {
            return Ok(None);
        };
        if b != ZPAD {
            continue;
        }

        let Some(b) = port.read_byte()? else {
            return Ok(None);
        };
        if b == ZDLE {
            return Ok(Some(()));
        }

        if b == ZPAD {
            let Some(b) = port.read_byte()? else {
                return Ok(None);
            };
            if b == ZDLE {
                return Ok(Some(()));
            }
        }
    }
}

/// Reads and unescapes a ZMODEM protocol subpacket.
///
/// # Errors
///
/// Returns `Error::Data` if the subpacket fails CRC validation or is malformed.
///
/// # Panics
///
/// The function will panic if the buffer is somehow empty when attempting to
/// pop the CRC byte, which should not happen in a valid transmission.
fn read_subpacket<P>(
    port: &mut P,
    buf: &mut Buffer<SUBPACKET_MAX_SIZE>,
    encoding: Encoding,
) -> Result<Option<Packet>, Error>
where
    P: Read + ?Sized,
{
    buf.clear();

    let (mut crc16, mut crc32) = (crc::Crc16::new(), crc::Crc32::new());

    let result = loop {
        let Some(byte) = port.read_byte()? else {
            return Ok(None);
        };
        if byte == ZDLE {
            let Some(byte) = port.read_byte()? else {
                return Ok(None);
            };
            if let Ok(packet) = Packet::try_from(byte) {
                if encoding == Encoding::ZBIN32 {
                    crc32.update_byte(packet as u8);
                } else {
                    crc16.update_byte(packet as u8);
                }
                break packet;
            }
            let unescaped = zdle::UNZDLE_TABLE[byte as usize];
            buf.push(unescaped).map_err(|_| Error::OutOfMemory)?;
            if encoding == Encoding::ZBIN32 {
                crc32.update_byte(unescaped);
            } else {
                crc16.update_byte(unescaped);
            }
        } else {
            buf.push(byte).map_err(|_| Error::OutOfMemory)?;
            if encoding == Encoding::ZBIN32 {
                crc32.update_byte(byte);
            } else {
                crc16.update_byte(byte);
            }
        }
    };

    let crc_len = if encoding == Encoding::ZBIN32 { 4 } else { 2 };
    let mut crc_bytes = [0u8; 4];
    for b in crc_bytes.iter_mut().take(crc_len) {
        let Some(byte) = read_byte_unescaped(port)? else {
            return Ok(None);
        };
        *b = byte;
    }

    if encoding == Encoding::ZBIN32 {
        if crc32.finalize().to_le_bytes() != crc_bytes {
            return Err(Error::UnexpectedCrc32);
        }
    } else if crc16.finalize().to_be_bytes() != [crc_bytes[0], crc_bytes[1]] {
        return Err(Error::UnexpectedCrc16);
    }

    Ok(Some(result))
}

/// Writes a subpacket.
///
/// # Errors
///
/// This function returns `Error::Write` on an I/O error or `Error::Data` on
/// a data validation error.
fn write_subpacket<P>(
    port: &mut P,
    encoding: Encoding,
    kind: Packet,
    data: &[u8],
) -> Result<Option<()>, Error>
where
    P: Write + ?Sized,
{
    let kind = kind as u8;
    if write_slice_escaped(port, data)?.is_none() {
        return Ok(None);
    }
    if port.write_byte(ZDLE)?.is_none() {
        return Ok(None);
    }
    if port.write_byte(kind)?.is_none() {
        return Ok(None);
    }
    match encoding {
        Encoding::ZBIN32 => {
            let mut crc = crc::Crc32::new();
            crc.update(data);
            crc.update_byte(kind);
            let buf = crc.finalize().to_le_bytes();
            write_slice_escaped(port, &buf)
        }
        Encoding::ZBIN => {
            let mut crc = crc::Crc16::new();
            crc.update(data);
            crc.update_byte(kind);
            let buf = crc.finalize().to_be_bytes();
            write_slice_escaped(port, &buf)
        }
        Encoding::ZHEX => Err(Error::Unsupported),
    }
}

fn check_crc(data: &[u8], crc: &[u8], encoding: Encoding) -> Result<(), Error> {
    let mut crc2 = [0u8; 4];
    let crc2_len = make_crc(data, &mut crc2, encoding);
    if *crc == crc2[..crc2_len] {
        Ok(())
    } else if encoding == Encoding::ZBIN32 {
        Err(Error::UnexpectedCrc32)
    } else {
        Err(Error::UnexpectedCrc16)
    }
}

fn make_crc(data: &[u8], out: &mut [u8], encoding: Encoding) -> usize {
    if encoding == Encoding::ZBIN32 {
        let crc = crate::crc::crc32_iso_hdlc(data).to_le_bytes();
        out[..4].copy_from_slice(&crc[..4]);
        4
    } else {
        let crc = crate::crc::crc16_xmodem(data).to_be_bytes();
        out[..2].copy_from_slice(&crc[..2]);
        2
    }
}

#[allow(dead_code)]
fn write_slice_escaped<P>(port: &mut P, buf: &[u8]) -> Result<Option<()>, Error>
where
    P: Write + ?Sized,
{
    for value in buf {
        if write_byte_escaped(port, *value)?.is_none() {
            return Ok(None);
        }
    }

    Ok(Some(()))
}

fn write_byte_escaped<P>(port: &mut P, value: u8) -> Result<Option<()>, Error>
where
    P: Write + ?Sized,
{
    let escaped = zdle::ZDLE_TABLE[value as usize];
    if escaped != value && port.write_byte(ZDLE)?.is_none() {
        return Ok(None);
    }
    port.write_byte(escaped)
}

fn read_byte_unescaped<P>(port: &mut P) -> Result<Option<u8>, Error>
where
    P: Read + ?Sized,
{
    let Some(b) = port.read_byte()? else {
        return Ok(None);
    };
    Ok(Some(if b == ZDLE {
        let Some(b) = port.read_byte()? else {
            return Ok(None);
        };
        zdle::UNZDLE_TABLE[b as usize]
    } else {
        b
    }))
}
