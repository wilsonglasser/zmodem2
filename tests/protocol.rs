// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2025 Jarkko Sakkinen

use core::cmp;
use rstest::rstest;
use zmodem2::{Encoding, Error, Frame, Header, Read, State, Transmission, Write, XON, ZDLE, ZPAD};

struct MockPort<'a> {
    input: &'a [u8],
    output: Vec<u8>,
    would_block: bool,
    block_next: bool,
}

impl<'a> MockPort<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            output: Vec::new(),
            would_block: false,
            block_next: false,
        }
    }

    fn with_would_block(mut self) -> Self {
        self.would_block = true;
        self
    }
}

impl<'a> Read for MockPort<'a> {
    fn read(&mut self, buf: &mut [u8]) -> Result<Option<u32>, Error> {
        if self.would_block && self.block_next {
            self.block_next = false;
            return Ok(None);
        }
        if self.input.is_empty() {
            return Ok(None);
        }
        let n = cmp::min(self.input.len(), buf.len());
        buf[..n].copy_from_slice(&self.input[..n]);
        self.input = &self.input[n..];
        if self.would_block {
            self.block_next = true;
        }
        Ok(Some(n as u32))
    }

    fn read_byte(&mut self) -> Result<Option<u8>, Error> {
        if self.would_block && self.block_next {
            self.block_next = false;
            return Ok(None);
        }
        if let Some((&first, rest)) = self.input.split_first() {
            self.input = rest;
            if self.would_block {
                self.block_next = true;
            }
            Ok(Some(first))
        } else {
            Ok(None)
        }
    }
}

impl<'a> Write for MockPort<'a> {
    fn write_all(&mut self, buf: &[u8]) -> Result<Option<()>, Error> {
        self.output.extend_from_slice(buf);
        Ok(Some(()))
    }

    fn write_byte(&mut self, value: u8) -> Result<Option<()>, Error> {
        self.output.push(value);
        Ok(Some(()))
    }
}

#[rstest]
#[case(Encoding::ZBIN, Frame::ZRQINIT, &[0; 4], &[ZPAD, ZDLE, Encoding::ZBIN as u8, 0, 0, 0, 0, 0, 0, 0])]
#[case(Encoding::ZBIN32, Frame::ZRQINIT, &[0; 4], &[ZPAD, ZDLE, Encoding::ZBIN32 as u8, 0, 0, 0, 0, 0, 29, 247, 34, 198])]
#[case(Encoding::ZBIN, Frame::ZRQINIT, &[1; 4], &[ZPAD, ZDLE, Encoding::ZBIN as u8, 0, 1, 1, 1, 1, 98, 148])]
#[case(Encoding::ZHEX, Frame::ZRQINIT, &[1; 4], &[ZPAD, ZPAD, ZDLE, Encoding::ZHEX as u8, b'0', b'0', b'0', b'1', b'0', b'1', b'0', b'1', b'0', b'1', 54, 50, 57, 52, b'\r', b'\n', XON])]
pub fn test_header_write(
    #[case] encoding: Encoding,
    #[case] frame: Frame,
    #[case] flags: &[u8; 4],
    #[case] expected: &[u8],
) {
    let header = Header::new(encoding, frame, flags);
    let mut port = vec![];
    assert!(header.write(&mut port) == Ok(Some(())));
    assert_eq!(port, expected);
}

#[rstest]
#[case(&[Encoding::ZHEX as u8, b'0', b'1', b'0', b'1', b'0', b'2', b'0', b'3', b'0', b'4', b'a', b'7', b'5', b'2'], Encoding::ZHEX, Frame::ZRINIT, &[0x1, 0x2, 0x3, 0x4])]
#[case(&[Encoding::ZBIN as u8, Frame::ZRINIT as u8, 0xa, 0xb, 0xc, 0xd, 0xa6, 0xcb], Encoding::ZBIN, Frame::ZRINIT, &[0xa, 0xb, 0xc, 0xd])]
#[case(&[Encoding::ZBIN32 as u8, Frame::ZRINIT as u8, 0xa, 0xb, 0xc, 0xd, 0x99, 0xe2, 0xae, 0x4a], Encoding::ZBIN32, Frame::ZRINIT, &[0xa, 0xb, 0xc, 0xd])]
#[case(&[Encoding::ZBIN as u8, Frame::ZRINIT as u8, 0xa, ZDLE, b'l', 0xd, ZDLE, b'm', 0x5e, 0x6f], Encoding::ZBIN, Frame::ZRINIT, &[0xa, 0x7f, 0xd, 0xff])]
pub fn test_header_read(
    #[case] port: &[u8],
    #[case] encoding: Encoding,
    #[case] frame: Frame,
    #[case] flags: &[u8; 4],
) {
    let port = &mut port.to_vec();
    let port = &mut port.as_slice();
    assert!(Header::read(port) == Ok(Some(Header::new(encoding, frame, flags))));
}

#[test]
fn test_receive_malformed_header() {
    let mut mock_port = MockPort::new(b"malformed data");
    let mut file = vec![];
    let mut state = Transmission::new();

    let result = state.receive(&mut mock_port, &mut file);
    assert!(matches!(result, Ok(None)));
}

#[test]
fn test_receive_zfile_with_non_utf8_name() {
    let file_name = b"bad\x80name";
    let file_size = 123;
    let mut sender = Transmission::new();
    sender.set_next_file_u8(file_name, file_size).unwrap();
    let mut send_port = MockPort::new(&[]);
    let mut file = std::io::Cursor::new(&[]);
    assert!(sender.send(&mut send_port, &mut file) == Ok(Some(())));

    let wire = send_port.output;
    let mut recv_port = MockPort::new(&wire).with_would_block();
    let mut sink = Vec::new();
    let mut receiver = Transmission::new();

    for _ in 0..(wire.len() * 4) {
        match receiver.receive(&mut recv_port, &mut sink) {
            Ok(Some(())) | Ok(None) => {}
            Err(e) => panic!("receive failed: {e}"),
        }
        if receiver.state() == State::FileBegin {
            break;
        }
    }

    assert_eq!(receiver.state(), State::FileBegin);
    assert_eq!(receiver.file_name(), file_name);
    assert_eq!(receiver.file_size(), file_size);
}
