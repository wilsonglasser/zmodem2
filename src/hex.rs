// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2023-2025 Jarkko Sakkinen

use crate::Error;

/// Encodes `data` into a hex string.
///
/// # Errors
///
/// Returns `Err(Error::CapacityExceeded)` if `output` is not large enough.
pub fn encode(data: &[u8], output: &mut [u8]) -> Result<(), Error> {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    let cap = data.len() * 2;
    if output.len() < cap {
        return Err(Error::CapacityExceeded(cap));
    }
    for (i, &byte) in data.iter().enumerate() {
        output[i * 2] = HEX_CHARS[((byte >> 4) & 0xF) as usize];
        output[i * 2 + 1] = HEX_CHARS[(byte & 0xF) as usize];
    }
    Ok(())
}

/// Decodes a hex string from `data` into `output`.
///
/// # Errors
///
/// Returns `Err(Error::InvalidHex)` if `data` is not valid hex string.
pub fn decode(data: &[u8], output: &mut [u8]) -> Result<(), Error> {
    if data.len() % 2 != 0 || output.len() < data.len() / 2 {
        return Err(Error::InvalidHex);
    }
    for i in 0..(data.len() / 2) {
        let high = data[i * 2];
        let low = data[i * 2 + 1];

        let high_val = match high {
            b'0'..=b'9' => high - b'0',
            b'a'..=b'f' => high - b'a' + 10,
            _ => return Err(Error::InvalidHex),
        };
        let low_val = match low {
            b'0'..=b'9' => low - b'0',
            b'a'..=b'f' => low - b'a' + 10,
            _ => return Err(Error::InvalidHex),
        };
        output[i] = (high_val << 4) | low_val;
    }
    Ok(())
}
