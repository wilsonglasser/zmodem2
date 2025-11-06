// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2023-2025 Jarkko Sakkinen

use crate::String;
use thiserror::Error;

/// I/O errors.
#[derive(Error, Debug, PartialEq)]
pub enum IoError {
    #[error("read: {0}")]
    Read(String),
    #[error("write: {0}")]
    Write(String),
    #[error("I/O operation would block")]
    WouldBlock,
    #[error("not connected")]
    NotConnected,
}

/// Protocol unmarshalling (parsing) errors.
#[derive(Error, Debug, PartialEq)]
pub enum UnmarshalError {
    #[error("{0}: unmarshal buffer capacity {1} exceeded")]
    CapacityExceeded(&'static str, usize),
    #[error("CRC-16 mismatch")]
    Crc16Mismatch,
    #[error("CRC-32 mismatch")]
    Crc32Mismatch,
    #[error("malformed encoding type: {0:#02x}")]
    MalformedEncoding(u8),
    #[error("malformed file size")]
    MalformedFileSize,
    #[error("malformed filename")]
    MalformedFileName,
    #[error("malformed frame type: {0:#02x}")]
    MalformedFrame(u8),
    #[error("malformed header")]
    MalformedHeader,
    #[error("malformed packet type: {0:#02x}")]
    MalformedPacket(u8),
}

/// Protocol marshalling (formatting) errors.
#[derive(Error, Debug, PartialEq)]
pub enum MarshalError {
    #[error("{0}: marshal buffer capacity {1} exceeded")]
    CapacityExceeded(&'static str, usize),
    #[error("file is truncated")]
    FileTruncated,
}

/// Main error type for the zmodem2 library.
#[derive(Error, Debug, PartialEq)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] IoError),
    #[error(transparent)]
    Marshal(#[from] MarshalError),
    #[error(transparent)]
    Unmarshal(#[from] UnmarshalError),
}
