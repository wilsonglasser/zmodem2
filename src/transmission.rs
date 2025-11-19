// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2025 Jarkko Sakkinen

//! ZMODEM transmission state and logic.

use crate::buffer::Buffer;
use crate::crc;
use crate::error::Error;
use crate::header::{
    read_byte_unescaped, write_slice_escaped, Encoding, Frame, Header, Zrinit, ZACK_HEADER,
    ZDATA_HEADER, ZEOF_HEADER, ZFIN_HEADER, ZNAK_HEADER, ZRPOS_HEADER, ZRQINIT_HEADER,
};
use crate::io::{Read, Seek, Write};
use crate::string::String;
use crate::zdle;
use crate::{ZDLE, ZPAD};
use core::fmt::Write as _;
use strum::EnumIter;
use strum::IntoEnumIterator;

/// Size of the unescaped subpacket payload. The size is picked from the
/// original ZMODEM specification.
const SUBPACKET_MAX_SIZE: usize = 1024;

/// The number of subpackets to stream
const SUBPACKET_PER_ACK: usize = 10;

/// The ZMODEM protocol subpacket type
#[repr(u8)]
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, EnumIter, PartialEq)]
pub enum SubpacketType {
    ZCRCE = 0x68,
    ZCRCG = 0x69,
    ZCRCQ = 0x6a,
    ZCRCW = 0x6b,
}

impl TryFrom<u8> for SubpacketType {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        SubpacketType::iter()
            .find(|e| value == *e as u8)
            .ok_or(Error::MalformedPacket(value))
    }
}

/// Internal state for reading a subpacket byte-by-byte
#[derive(Clone, Copy, Debug, PartialEq)]
enum SubpacketState {
    Idle,
    Data,
    Crc(SubpacketType),
}

/// Send or receive transmission state
pub struct Transmission {
    state: State,
    count: u32,
    file_name: String,
    file_size: u32,
    buf: Buffer<SUBPACKET_MAX_SIZE>,
    data_encoding: Encoding,
    subpacket_state: SubpacketState,
    crc_calculator_16: crc::Crc16,
    crc_calculator_32: crc::Crc32,
    crc_bytes_read: u8,
    crc_buf: [u8; 4],
}

impl Default for Transmission {
    fn default() -> Self {
        Self::new()
    }
}

impl Transmission {
    /// Create a new transmission context
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: State::SessionBegin,
            count: 0,
            file_name: String::new(),
            file_size: 0,
            buf: Buffer::<SUBPACKET_MAX_SIZE>::new(),
            data_encoding: Encoding::ZBIN,
            subpacket_state: SubpacketState::Idle,
            crc_calculator_16: crc::Crc16::new(),
            crc_calculator_32: crc::Crc32::new(),
            crc_bytes_read: 0,
            crc_buf: [0; 4],
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
        self.state = State::FileEnd;
        Ok(())
    }

    #[must_use]
    pub fn state(&self) -> State {
        self.state
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
        if self.state == State::SessionBegin {
            if ZRQINIT_HEADER.write(port)?.is_none() {
                return Ok(None);
            }
            self.state = State::SessionSyncing;
            return Ok(Some(()));
        } else if self.state == State::FileEnd {
            if write_zfile(port, &mut self.buf, &self.file_name, self.file_size)?.is_none() {
                return Ok(None);
            }
            self.state = State::FileBegin;
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
                if self.state == State::SessionSyncing {
                    if write_zfile(port, &mut self.buf, &self.file_name, self.file_size)?.is_none()
                    {
                        return Ok(None);
                    }
                    self.state = State::FileBegin;
                } else if self.state == State::FileWaitingSubpacket {
                    self.state = State::FileEnd;
                }
            }
            Frame::ZRPOS | Frame::ZACK => {
                if self.state == State::SessionSyncing {
                    if ZRQINIT_HEADER.write(port)?.is_none() {
                        return Ok(None);
                    }
                } else if self.state == State::FileBegin
                    || self.state == State::FileWaitingSubpacket
                {
                    if write_zdata(port, file, header.count())?.is_none() {
                        return Ok(None);
                    }
                    self.state = State::FileWaitingSubpacket;
                }
            }
            _ => {
                if self.state == State::SessionSyncing && ZRQINIT_HEADER.write(port)?.is_none() {
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
        if self.state == State::FileReadingSubpacket {
            return self.receive_subpacket(port, file);
        }
        if self.state == State::FileReadingMetadata {
            return self.receive_subpacket_metadata(port);
        }

        if self.state == State::SessionBegin && write_zrinit(port)?.is_none() {
            return Ok(None);
        }

        let read_zpad_result = read_zpad(port);
        let Some(()) = (match read_zpad_result {
            Err(Error::Read(err)) => {
                if self.state == State::FileBegin {
                    self.state = State::SessionEnd;
                    return Ok(Some(()));
                }
                Err(Error::Read(err))
            }
            r => r,
        })?
        else {
            return Ok(None);
        };

        let header = match Header::read(port) {
            Ok(Some(header)) => header,
            Ok(None) => return Ok(None),
            Err(Error::Read(err)) => {
                if self.state == State::FileBegin {
                    self.state = State::SessionEnd;
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
                if self.state == State::SessionBegin || self.state == State::FileBegin {
                    self.data_encoding = header.encoding();
                    self.state = State::FileReadingMetadata;
                    self.subpacket_state = SubpacketState::Data;
                    self.crc_calculator_16 = crc::Crc16::new();
                    self.crc_calculator_32 = crc::Crc32::new();
                    self.buf.clear();
                }
            }
            Frame::ZDATA => {
                if self.state == State::SessionBegin {
                    if write_zrinit(port)?.is_none() {
                        return Ok(None);
                    }
                } else if self.state == State::FileBegin
                    || self.state == State::FileWaitingSubpacket
                {
                    if header.count() != self.count {
                        if ZRPOS_HEADER.with_count(self.count).write(port)?.is_none() {
                            return Ok(None);
                        }
                        return Ok(Some(()));
                    }
                    self.data_encoding = header.encoding();
                    self.state = State::FileReadingSubpacket;
                    self.subpacket_state = SubpacketState::Data;
                    self.crc_calculator_16 = crc::Crc16::new();
                    self.crc_calculator_32 = crc::Crc32::new();
                    self.buf.clear();
                }
            }
            Frame::ZEOF => {
                if self.state == State::FileWaitingSubpacket && header.count() == self.count {
                    if write_zrinit(port)?.is_none() {
                        return Ok(None);
                    }
                    self.state = State::FileBegin;
                }
            }
            Frame::ZFIN => {
                if self.state == State::FileWaitingSubpacket || self.state == State::FileBegin {
                    if ZFIN_HEADER.write(port)?.is_none() {
                        return Ok(None);
                    }
                    self.state = State::SessionEnd;
                }
            }
            _ => {}
        }
        Ok(Some(()))
    }

    /// Parses the file info buffer after a ZFILE subpacket is received.
    fn parse_zfile_buf(&mut self) -> Result<(), Error> {
        let payload = &self.buf;
        let mut fields = payload.split(|&b| b == b'\0');

        let file_name_bytes = fields.next().ok_or(Error::MalformedFileName)?;
        if file_name_bytes.is_empty() {
            return Err(Error::MalformedFileName);
        }

        core::str::from_utf8(file_name_bytes).map_err(|_| Error::MalformedFileName)?;

        self.file_name.clear();
        self.file_name
            .extend_from_slice(file_name_bytes)
            .map_err(|_| Error::OutOfMemory)?;

        if let Some(size_str_bytes) = fields.next() {
            let size_field_bytes = size_str_bytes
                .split(|&b| b == b' ')
                .next()
                .unwrap_or_default();

            self.file_size = parse_file_size(size_field_bytes)?;
        } else {
            self.file_size = 0;
        }

        self.count = 0;
        Ok(())
    }

    /// Handles reading a single byte for the `SubpacketState::Data` state.
    fn receive_subpacket_data_byte<P>(&mut self, port: &mut P) -> Result<Option<()>, Error>
    where
        P: Read + Write + ?Sized,
    {
        let Some(byte) = port.read_byte()? else {
            return Ok(None);
        };

        if byte == ZDLE {
            let Some(byte) = port.read_byte()? else {
                return Ok(None);
            };
            if let Ok(packet) = SubpacketType::try_from(byte) {
                if self.data_encoding == Encoding::ZBIN32 {
                    self.crc_calculator_32.update_byte(packet as u8);
                } else {
                    self.crc_calculator_16.update_byte(packet as u8);
                }
                self.subpacket_state = SubpacketState::Crc(packet);
                self.crc_bytes_read = 0;
                self.crc_buf = [0; 4];
            } else {
                let unescaped = zdle::UNZDLE_TABLE[byte as usize];
                self.buf.push(unescaped).map_err(|_| Error::OutOfMemory)?;
                if self.data_encoding == Encoding::ZBIN32 {
                    self.crc_calculator_32.update_byte(unescaped);
                } else {
                    self.crc_calculator_16.update_byte(unescaped);
                }
            }
        } else {
            self.buf.push(byte).map_err(|_| Error::OutOfMemory)?;
            if self.data_encoding == Encoding::ZBIN32 {
                self.crc_calculator_32.update_byte(byte);
            } else {
                self.crc_calculator_16.update_byte(byte);
            }
        }
        Ok(Some(()))
    }

    /// Handles the byte-by-byte reading of a ZFILE info subpacket.
    fn receive_subpacket_metadata<P>(&mut self, port: &mut P) -> Result<Option<()>, Error>
    where
        P: Read + Write + ?Sized,
    {
        match self.subpacket_state {
            SubpacketState::Data => self.receive_subpacket_data_byte(port),
            SubpacketState::Crc(_) => {
                let crc_len = if self.data_encoding == Encoding::ZBIN32 {
                    4
                } else {
                    2
                };

                let Some(byte) = read_byte_unescaped(port)? else {
                    return Ok(None);
                };
                self.crc_buf[self.crc_bytes_read as usize] = byte;
                self.crc_bytes_read += 1;

                if self.crc_bytes_read < crc_len {
                    return Ok(Some(()));
                }

                if self.data_encoding == Encoding::ZBIN32 {
                    let expected_crc = self.crc_calculator_32.finalize().to_le_bytes();
                    if expected_crc != self.crc_buf {
                        return Err(Error::UnexpectedCrc32);
                    }
                } else {
                    let expected_crc = self.crc_calculator_16.finalize().to_be_bytes();
                    if expected_crc != [self.crc_buf[0], self.crc_buf[1]] {
                        return Err(Error::UnexpectedCrc16);
                    }
                }

                self.parse_zfile_buf()?;
                self.buf.clear();
                self.crc_calculator_16 = crc::Crc16::new();
                self.crc_calculator_32 = crc::Crc32::new();

                if ZRPOS_HEADER.with_count(0).write(port)?.is_none() {
                    return Ok(None);
                }

                self.state = State::FileBegin;
                self.subpacket_state = SubpacketState::Idle;
                Ok(Some(()))
            }
            SubpacketState::Idle => Err(Error::Unsupported),
        }
    }

    /// Handles the byte-by-byte reading of a ZDATA subpacket.
    fn receive_subpacket<P, F>(&mut self, port: &mut P, file: &mut F) -> Result<Option<()>, Error>
    where
        P: Read + Write + ?Sized,
        F: Write + ?Sized,
    {
        match self.subpacket_state {
            SubpacketState::Data => self.receive_subpacket_data_byte(port),
            SubpacketState::Crc(packet) => {
                let crc_len = if self.data_encoding == Encoding::ZBIN32 {
                    4
                } else {
                    2
                };

                let Some(byte) = read_byte_unescaped(port)? else {
                    return Ok(None);
                };
                self.crc_buf[self.crc_bytes_read as usize] = byte;
                self.crc_bytes_read += 1;

                if self.crc_bytes_read < crc_len {
                    return Ok(Some(()));
                }

                if self.data_encoding == Encoding::ZBIN32 {
                    let expected_crc = self.crc_calculator_32.finalize().to_le_bytes();
                    if expected_crc != self.crc_buf {
                        return Err(Error::UnexpectedCrc32);
                    }
                } else {
                    let expected_crc = self.crc_calculator_16.finalize().to_be_bytes();
                    if expected_crc != [self.crc_buf[0], self.crc_buf[1]] {
                        return Err(Error::UnexpectedCrc16);
                    }
                }

                if file.write_all(&self.buf)?.is_none() {
                    return Ok(None);
                }
                self.count += u32::try_from(self.buf.len()).map_err(|_| Error::OutOfMemory)?;
                self.buf.clear();
                self.crc_calculator_16 = crc::Crc16::new();
                self.crc_calculator_32 = crc::Crc32::new();

                match packet {
                    SubpacketType::ZCRCW | SubpacketType::ZCRCQ => {
                        if ZACK_HEADER.with_count(self.count).write(port)?.is_none() {
                            return Ok(None);
                        }
                        self.subpacket_state = SubpacketState::Data;
                    }
                    SubpacketType::ZCRCG => {
                        self.subpacket_state = SubpacketState::Data;
                    }
                    SubpacketType::ZCRCE => {
                        self.state = State::FileWaitingSubpacket;
                        self.subpacket_state = SubpacketState::Idle;
                    }
                }
                Ok(Some(()))
            }
            SubpacketState::Idle => Err(Error::Unsupported),
        }
    }

    /// Send ZFIN.
    ///
    /// # Errors
    ///
    /// * [`Read`](crate::Error::Read) when the read I/O fails with the serial port.
    /// * [`Write`](crate::Error::Write) when the write I/O fails with the serial port.
    /// * [`Data`](crate::Error::Data) when corrupted data has been detected.
    pub fn finish<P>(&mut self, port: &mut P) -> Result<Option<()>, Error>
    where
        P: Read + Write + ?Sized,
    {
        if self.state != State::SessionTeardown {
            if ZFIN_HEADER.write(port)?.is_none() {
                return Ok(None);
            }
            self.state = State::SessionTeardown;
            return Ok(Some(()));
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
                self.state = State::SessionEnd;
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
pub enum State {
    FileBegin,
    FileEnd,
    FileReadingMetadata,
    FileReadingSubpacket,
    FileWaitingSubpacket,
    SessionBegin,
    SessionEnd,
    SessionSyncing,
    SessionTeardown,
}

/// Writes ZRINIT
fn write_zrinit<P>(port: &mut P) -> Result<Option<()>, Error>
where
    P: Write + ?Sized,
{
    let zrinit = Zrinit::CANFDX | Zrinit::CANOVIO | Zrinit::CANFC32;
    Header::new(Encoding::ZHEX, Frame::ZRINIT, &[0, 0, 0, zrinit.bits()]).write(port)
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
    buf.clear();
    buf.extend_from_slice(name)
        .map_err(|_| Error::OutOfMemory)?;
    buf.push(b'\0').map_err(|_| Error::OutOfMemory)?;

    write!(buf, "{size}\0").map_err(|_| Error::OutOfMemory)?;

    if Header::new(Encoding::ZBIN32, Frame::ZFILE, &[0; 4])
        .write(port)?
        .is_none()
    {
        return Ok(None);
    }
    write_subpacket(port, Encoding::ZBIN32, SubpacketType::ZCRCW, buf)
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
        if write_subpacket(
            port,
            Encoding::ZBIN32,
            SubpacketType::ZCRCG,
            &read_buf[..count],
        )?
        .is_none()
        {
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
    write_subpacket(
        port,
        Encoding::ZBIN32,
        SubpacketType::ZCRCW,
        &read_buf[..count],
    )
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
    let Some(b) = port.read_byte()? else {
        return Ok(None);
    };
    if b != ZPAD {
        return Ok(None);
    }

    let Some(b) = port.read_byte()? else {
        return Ok(None);
    };
    if b == ZDLE {
        return Ok(Some(()));
    }
    if b != ZPAD {
        return Ok(None);
    }

    let Some(b) = port.read_byte()? else {
        return Ok(None);
    };
    if b == ZDLE {
        return Ok(Some(()));
    }

    Ok(None)
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
    kind: SubpacketType,
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
