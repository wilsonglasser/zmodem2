// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2025 Jarkko Sakkinen

//! ZMODEM file transfer protocol crate. `zmodem2::State::receive` and `zmodem2::State::send`
//! provide a synchronous and sequential API for sending and receiving files
//! with the ZMODEM protocol. Each step corresponds to a single ZMODEM frame
//! transaction, and the state between the calls is kept in a `zmodem2::State`
//! instance.
//! The usage can be described in the high-level with the following flow:
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
mod header;
mod io;
#[cfg(feature = "std")]
mod std;
mod string;
mod transmission;
mod zdle;

pub use buffer::*;
pub use error::*;
pub use header::*;
pub use io::*;
pub use string::*;
pub use transmission::*;

pub const ZPAD: u8 = b'*';
pub const ZDLE: u8 = 0x18;
pub const XON: u8 = 0x11;
