// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2025 Jarkko Sakkinen

//! ZMODEM file transfer protocol crate. `zmodem2::receive` and `zmodem2::send`
//! provide a synchronous and sequential API for sending and receiving files
//! with the ZMODEM protocol. Each step corresponds to a single ZMODEM frame
//! transaction, and the state between the calls is kept in a `zmodem2::State`
//! instance.
//!
//! The usage can be described in the high-level with the following flow:
//!
//! 1. Create `zmodem2::State`.
//! 2. Call either `zmodem2::send` or `zmodem2::receive`.
//! 3. If the returned `zmodem2::Stage` is not yet `zmodem2::Stage::Done`, go
//!    back to step 2.

#![deny(clippy::all)]
#![deny(clippy::pedantic)]
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "std")]
mod std;
mod zdle;

use bitflags::bitflags;
use core::{convert::TryFrom, str::FromStr};
use crc::{Crc, CRC_16_XMODEM, CRC_32_ISO_HDLC};
use heapless::String;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use tinyvec::{array_vec, ArrayVec};

/// Size of the unescaped subpacket payload. The size was picked based on
/// maximum subpacket size in the original 1988 ZMODEM specification.
const BUFFER_SIZE: usize = 1024;

/// Buffer size with enough capacity for an escaped header
const HEADER_SIZE: usize = 32;

/// The number of subpackets to stream
const SUBPACKET_PER_ACK: usize = 10;

/// CRC algorithm for `ZBIN` or `ZHEX` encoded transmissions.
const CRC16: Crc<u16> = Crc::<u16>::new(&CRC_16_XMODEM);

/// CRC algorithm for `ZBIN32` encoded transmissions.
const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);

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

/// Staging and temporal storage for incoming and outgoing frames
pub type Buffer = ArrayVec<[u8; BUFFER_SIZE]>;

/// Error codes for `zmodem2::send` and `zmodem2::receive`
#[derive(Debug, PartialEq)]
pub enum Error {
    /// The received data failed validation
    Data,
    /// The field for filename is missing in the ZFILE packet
    FileNameMissing,
    /// Filename in the ZFILE packet has zero length
    FileNameEmpty,
    /// I/O error during read
    Read,
    /// I/O error during write
    Write,
}

/// Write I/O operations
pub trait Write {
    /// Attempts to write the entire buffer
    ///
    /// # Errors
    ///
    /// * `Err(Error::Read)` when the read I/O fails with the serial port
    /// * `Err(Error::Write)` when the write I/O fails with the serial port
    /// * `Err(Error::Data)` when corrupted data has been detected
    fn write_all(&mut self, buf: &[u8]) -> Result<(), Error>;

    /// Attempts to write a single byte
    ///
    /// # Errors
    ///
    /// * `Err(Error::Read)` when the read I/O fails with the serial port
    /// * `Err(Error::Write)` when the write I/O fails with the serial port
    /// * `Err(Error::Data)` when corrupted data has been detected
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
    fn read(&mut self, buf: &mut [u8]) -> Result<u32, Error>;

    /// Reads exactly one byte to the buffer
    ///
    /// # Errors
    ///
    /// * `Err(Error::Read)` when the read I/O fails with the serial port
    /// * `Err(Error::Write)` when the write I/O fails with the serial port
    /// * `Err(Error::Data)` when corrupted data has been detected
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
    fn seek(&mut self, offset: u32) -> Result<(), Error>;
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
    /// * `Err(Error::Read)` when the read I/O fails with the serial port
    /// * `Err(Error::Write)` when the write I/O fails with the serial port
    /// * `Err(Error::Data)` when corrupted data has been detected
    pub fn write<P>(&self, port: &mut P) -> Result<(), Error>
    where
        P: Write,
    {
        let mut out = array_vec!([u8; HEADER_SIZE]);
        port.write_byte(ZPAD)?;
        if self.encoding == Encoding::ZHEX {
            port.write_byte(ZPAD)?;
        }
        port.write_byte(ZDLE)?;
        port.write_byte(self.encoding as u8)?;
        out.push(self.frame as u8);
        out.extend_from_slice(&self.flags);
        // Skips ZPAD and encoding:
        let mut crc = [0u8; 4];
        let crc_len = make_crc(&out, &mut crc, self.encoding);
        out.extend_from_slice(&crc[..crc_len]);
        // Skips ZPAD and encoding:
        if self.encoding == Encoding::ZHEX {
            let mut hexbuf = [0u8; HEADER_SIZE];
            let len = out.len() * 2;
            if len > hexbuf.len() {
                return Err(Error::Data);
            }
            let hex = &mut hexbuf[..len];
            hex::encode_to_slice(out, hex).map_err(|_| Error::Data)?;
            out.truncate(0);
            out.extend_from_slice(hex);
        }
        write_slice_escaped(port, &out)?;
        if self.encoding == Encoding::ZHEX {
            // Add trailing CRLF for ZHEX transfer:
            port.write_byte(b'\r')?;
            port.write_byte(b'\n')?;
            if self.frame != Frame::ZACK && self.frame != Frame::ZFIN {
                port.write_byte(XON)?;
            }
        }
        Ok(())
    }

    /// Reads and decodes a header from the serial port, and returns a new
    /// instance
    ///
    /// # Errors
    ///
    /// * `Err(Error::Read)` when the read I/O fails with the serial port
    /// * `Err(Error::Write)` when the write I/O fails with the serial port
    /// * `Err(Error::Data)` when corrupted data has been detected
    pub fn read<P>(port: &mut P) -> Result<Header, Error>
    where
        P: Read,
    {
        let encoding = Encoding::try_from(port.read_byte()?)?;
        let mut out_hex = array_vec!([u8; HEADER_SIZE]);
        for _ in 0..Header::unescaped_size(encoding) - 1 {
            out_hex.push(read_byte_unescaped(port)?);
        }
        let mut out = array_vec!([u8; HEADER_SIZE]);
        out.set_len(out_hex.len() / 2);
        if encoding == Encoding::ZHEX {
            hex::decode_to_slice(out_hex, &mut out).map_err(|_| Error::Data)?;
        } else {
            out = out_hex;
        }
        check_crc(&out[..5], &out[5..], encoding)?;
        let frame = Frame::try_from(out[0])?;
        let mut header = Header::new(encoding, frame, &[0; 4]);
        header.flags.copy_from_slice(&out[1..=4]);
        Ok(header)
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
            // Encoding is stored as a single byte also for ZHEX, thus the
            // subtraction:
            Encoding::ZHEX => (core::mem::size_of::<Header>() + 2) * 2 - 1,
        }
    }
}

/// The ZMODEM protocol frame encoding
#[repr(u8)]
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, EnumIter, PartialEq)]
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
            .ok_or(Error::Data)
    }
}

#[repr(u8)]
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, EnumIter, PartialEq)]
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
        Frame::iter().find(|t| value == *t as u8).ok_or(Error::Data)
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
#[derive(Clone, Copy, EnumIter, PartialEq)]
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
            .ok_or(Error::Data)
    }
}

/// Send or receive transmission state
pub struct State {
    stage: Stage,
    count: u32,
    file_name: String<256>,
    file_size: u32,
    buf: Buffer,
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
            stage: Stage::Waiting,
            count: 0,
            file_name: String::new(),
            file_size: 0,
            buf: Buffer::from_array_empty([0; BUFFER_SIZE]),
        }
    }

    /// Create a new transmission context with file name and size
    ///
    /// # Errors
    ///
    /// * `Err(Error::Read)` when the read I/O fails with the serial port
    /// * `Err(Error::Write)` when the write I/O fails with the serial port
    /// * `Err(Error::Data)` when corrupted data has been detected
    pub fn new_file(file_name: &str, file_size: u32) -> Result<Self, Error> {
        let file_name = String::from_str(file_name).or(Err(Error::Data))?;
        Ok(Self {
            stage: Stage::Waiting,
            count: 0,
            file_name,
            file_size,
            buf: Buffer::from_array_empty([0; BUFFER_SIZE]),
        })
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
    pub fn file_name(&self) -> &str {
        &self.file_name
    }

    #[must_use]
    pub fn file_size(&self) -> u32 {
        self.file_size
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Stage {
    Waiting,
    Ready,
    InProgress,
    Done,
}

/// Sends a file using the ZMODEM file transfer protocol.
///
/// # Errors
///
/// * `Err(Error::Read)` when the read I/O fails with the serial port
/// * `Err(Error::Write)` when the write I/O fails with the serial port
/// * `Err(Error::Data)` when corrupted data has been detected
pub fn send<P, F>(port: &mut P, file: &mut F, state: &mut State) -> Result<(), Error>
where
    P: Read + Write,
    F: Read + Seek,
{
    if state.stage == Stage::Waiting {
        ZRQINIT_HEADER.write(port)?;
    }
    if read_zpad(port).is_err() {
        return Ok(());
    }
    let Ok(frame) = Header::read(port) else {
        ZNAK_HEADER.write(port)?;
        return Ok(());
    };
    match frame.frame() {
        Frame::ZRINIT => {
            if state.stage == Stage::Waiting {
                write_zfile(port, &mut state.buf, &state.file_name, state.file_size)?;
                state.stage = Stage::Ready;
            } else if state.stage == Stage::InProgress {
                ZFIN_HEADER.write(port)?;
            }
        }
        Frame::ZRPOS | Frame::ZACK => {
            if state.stage == Stage::Waiting {
                ZRQINIT_HEADER.write(port)?;
            } else if state.stage == Stage::Ready || state.stage == Stage::InProgress {
                write_zdata(port, &mut state.buf, file, frame.count())?;
                state.stage = Stage::InProgress;
            }
        }
        Frame::ZFIN => {
            if state.stage == Stage::InProgress {
                port.write_byte(b'O')?;
                port.write_byte(b'O')?;
                state.stage = Stage::Done;
            }
        }
        _ => {
            if state.stage == Stage::Waiting {
                ZRQINIT_HEADER.write(port)?;
            }
        }
    }
    Ok(())
}

/// Receives a file using the ZMODEM file transfer protocol.
///
/// # Errors
///
/// * `Err(Error::Read)` when the read I/O fails with the serial port
/// * `Err(Error::Write)` when the write I/O fails with the serial port
/// * `Err(Error::Data)` when corrupted data has been detected
pub fn receive<P, F>(port: &mut P, file: &mut F, state: &mut State) -> Result<(), Error>
where
    P: Read + Write,
    F: Write,
{
    if state.stage == Stage::Waiting {
        write_zrinit(port)?;
    }
    if read_zpad(port).is_err() {
        return Ok(());
    }
    let Ok(header) = Header::read(port) else {
        ZNAK_HEADER.write(port)?;
        return Ok(());
    };
    match header.frame() {
        Frame::ZFILE => {
            if state.stage == Stage::Waiting || state.stage == Stage::Ready {
                read_zfile(port, state, header.encoding())?;
                state.stage = Stage::Ready;
            }
        }
        Frame::ZDATA => {
            if state.stage == Stage::Waiting {
                write_zrinit(port)?;
            } else if state.stage == Stage::Ready || state.stage == Stage::InProgress {
                if header.count() != state.count {
                    ZRPOS_HEADER.with_count(state.count).write(port)?;
                    return Ok(());
                }
                read_zdata(port, state, header.encoding(), file)?;
                state.stage = Stage::InProgress;
            }
        }
        Frame::ZEOF => {
            if state.stage == Stage::InProgress && header.count() == state.count {
                write_zrinit(port)?;
            }
        }
        Frame::ZFIN => {
            if state.stage == Stage::InProgress {
                ZFIN_HEADER.write(port)?;
                state.stage = Stage::Done;
            }
        }
        _ => (),
    }
    Ok(())
}

/// Writes ZRINIT
fn write_zrinit<P>(port: &mut P) -> Result<(), Error>
where
    P: Write,
{
    let zrinit = Zrinit::CANFDX | Zrinit::CANOVIO | Zrinit::CANFC32;
    Header::new(Encoding::ZHEX, Frame::ZRINIT, &[0, 0, 0, zrinit.bits()]).write(port)
}

/// Write ZRFILE
fn write_zfile<P>(port: &mut P, buf: &mut Buffer, name: &str, size: u32) -> Result<(), Error>
where
    P: Write,
{
    let size = String::<17>::try_from(size).or(Err(Error::Data))?;
    buf.clear();
    buf.extend_from_slice(name.as_bytes());
    buf.push(b'\0');
    buf.extend_from_slice(size.as_ref());
    buf.push(b'\0');
    Header::new(Encoding::ZBIN32, Frame::ZFILE, &[0; 4]).write(port)?;
    write_subpacket(port, Encoding::ZBIN32, Packet::ZCRCW, buf)
}

/// Parses filename and size from the subpacket sent after the `Frame::ZFiLE`
/// header.
fn read_zfile<P>(port: &mut P, state: &mut State, encoding: Encoding) -> Result<(), Error>
where
    P: Read + Write,
{
    match read_subpacket(port, &mut state.buf, encoding) {
        Ok(_) => {
            let payload = core::str::from_utf8(state.buf.as_slice()).map_err(|_| Error::Data)?;
            let mut fields = payload.split('\0');

            let file_name = fields.next().ok_or(Error::FileNameMissing)?;
            if file_name.is_empty() {
                return Err(Error::FileNameEmpty);
            }
            state.file_name = String::from_str(file_name).map_err(|()| Error::Data)?;

            if let Some(size_str) = fields.next() {
                if let Some(field) = size_str.split_ascii_whitespace().next() {
                    state.file_size = u32::from_str(field).map_err(|_| Error::Data)?;
                }
            }

            ZRPOS_HEADER.with_count(0).write(port)
        }
        _ => ZNAK_HEADER.write(port).map_err(|_| Error::Data),
    }
}

/// Writes ZDATA
fn write_zdata<P, F>(port: &mut P, buf: &mut Buffer, file: &mut F, offset: u32) -> Result<(), Error>
where
    P: Read + Write,
    F: Read + Seek,
{
    let mut offset = offset;
    buf.set_len(BUFFER_SIZE - 2);
    file.seek(offset)?;
    let mut count: u32 = file.read(buf)?;
    if count == 0 {
        ZEOF_HEADER.with_count(offset).write(port)?;
        return Ok(());
    }
    ZDATA_HEADER.with_count(offset).write(port)?;
    for _ in 1..SUBPACKET_PER_ACK {
        write_subpacket(
            port,
            Encoding::ZBIN32,
            Packet::ZCRCG,
            &buf[..count as usize],
        )?;
        offset += count;

        count = file.read(buf)?;
        if (count as usize) < buf.len() {
            break;
        }
    }
    write_subpacket(
        port,
        Encoding::ZBIN32,
        Packet::ZCRCW,
        &buf[..count as usize],
    )
}

/// Reads ZDATA
fn read_zdata<P, F>(
    port: &mut P,
    state: &mut State,
    encoding: Encoding,
    file: &mut F,
) -> Result<(), Error>
where
    P: Read + Write,
    F: Write,
{
    loop {
        let zcrc = match read_subpacket(port, &mut state.buf, encoding) {
            Ok(zcrc) => {
                if state.buf.is_empty() {
                    ZRPOS_HEADER.with_count(state.count).write(port)?;
                }
                zcrc
            }
            Err(Error::Data) => {
                ZNAK_HEADER.with_count(state.count).write(port)?;
                continue;
            }
            Err(err) => return Err(err),
        };
        file.write_all(&state.buf)?;
        state.count += u32::try_from(state.buf.len()).map_err(|_| Error::Data)?;
        match zcrc {
            Packet::ZCRCW => {
                ZACK_HEADER.with_count(state.count).write(port)?;
                return Ok(());
            }
            Packet::ZCRCE => return Ok(()),
            Packet::ZCRCQ => {
                ZACK_HEADER.with_count(state.count).write(port)?;
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
pub fn read_zpad<P>(port: &mut P) -> Result<(), Error>
where
    P: Read,
{
    if port.read_byte()? != ZPAD {
        return Err(Error::Data);
    }

    let mut b = port.read_byte()?;
    if b == ZPAD {
        b = port.read_byte()?;
    }

    if b == ZDLE {
        return Ok(());
    }

    Err(Error::Data)
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
pub fn read_subpacket<P>(
    port: &mut P,
    buf: &mut Buffer,
    encoding: Encoding,
) -> Result<Packet, Error>
where
    P: Read,
{
    buf.clear();
    let result = loop {
        let byte = port.read_byte()?;
        if byte == ZDLE {
            let byte = port.read_byte()?;
            if let Ok(packet) = Packet::try_from(byte) {
                buf.push(packet as u8);
                break packet;
            }
            buf.push(zdle::UNZDLE_TABLE[byte as usize]);
        } else {
            buf.push(byte);
        }

        if buf.len() == buf.capacity() {
            let packet = skip_subpacket_tail(port, encoding)?;
            buf.set_len(0);
            return Ok(packet);
        }
    };

    let crc_len = if encoding == Encoding::ZBIN32 { 4 } else { 2 };
    let mut crc = [0u8; 4];
    for b in crc.iter_mut().take(crc_len) {
        *b = read_byte_unescaped(port)?;
    }
    check_crc(buf, &crc[..crc_len], encoding)?;

    // Pop ZCRC
    buf.pop().unwrap();
    Ok(result)
}

/// Skips the tail of the subpacket (including CRC).
fn skip_subpacket_tail<P>(port: &mut P, encoding: Encoding) -> Result<Packet, Error>
where
    P: Read,
{
    let result;
    loop {
        let byte = port.read_byte()?;
        if byte == ZDLE {
            let byte = port.read_byte()?;
            if let Ok(packet) = Packet::try_from(byte) {
                result = packet;
                break;
            }
        }
    }
    let crc_len = if encoding == Encoding::ZBIN32 { 4 } else { 2 };
    for _ in 0..crc_len {
        read_byte_unescaped(port)?;
    }
    Ok(result)
}

/// Writes a subpacket.
///
/// # Errors
///
/// This function returns `Error::Write` on an I/O error or `Error::Data` on
/// a data validation error.
pub fn write_subpacket<P>(
    port: &mut P,
    encoding: Encoding,
    kind: Packet,
    data: &[u8],
) -> Result<(), Error>
where
    P: Write,
{
    let kind = kind as u8;
    write_slice_escaped(port, data)?;
    port.write_byte(ZDLE)?;
    port.write_byte(kind)?;
    match encoding {
        Encoding::ZBIN32 => {
            let mut digest = CRC32.digest();
            digest.update(data);
            digest.update(&[kind]);
            write_slice_escaped(port, &digest.finalize().to_le_bytes())
        }
        Encoding::ZBIN => {
            let mut digest = CRC16.digest();
            digest.update(data);
            digest.update(&[kind]);
            write_slice_escaped(port, &digest.finalize().to_be_bytes())
        }
        Encoding::ZHEX => {
            unimplemented!()
        }
    }
}

fn check_crc(data: &[u8], crc: &[u8], encoding: Encoding) -> Result<(), Error> {
    let mut crc2 = [0u8; 4];
    let crc2_len = make_crc(data, &mut crc2, encoding);
    if *crc == crc2[..crc2_len] {
        Ok(())
    } else {
        Err(Error::Data)
    }
}

fn make_crc(data: &[u8], out: &mut [u8], encoding: Encoding) -> usize {
    if encoding == Encoding::ZBIN32 {
        let crc = CRC32.checksum(data).to_le_bytes();
        out[..4].copy_from_slice(&crc[..4]);
        4
    } else {
        let crc = CRC16.checksum(data).to_be_bytes();
        out[..2].copy_from_slice(&crc[..2]);
        2
    }
}

#[allow(dead_code)]
fn write_slice_escaped<P>(port: &mut P, buf: &[u8]) -> Result<(), Error>
where
    P: Write,
{
    for value in buf {
        write_byte_escaped(port, *value)?;
    }

    Ok(())
}

fn write_byte_escaped<P>(port: &mut P, value: u8) -> Result<(), Error>
where
    P: Write,
{
    let escaped = zdle::ZDLE_TABLE[value as usize];
    if escaped != value {
        port.write_byte(ZDLE)?;
    }
    port.write_byte(escaped)
}

fn read_byte_unescaped<P>(port: &mut P) -> Result<u8, Error>
where
    P: Read,
{
    let b = port.read_byte()?;
    Ok(if b == ZDLE {
        zdle::UNZDLE_TABLE[port.read_byte()? as usize]
    } else {
        b
    })
}
