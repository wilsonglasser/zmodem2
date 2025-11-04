// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2025 Jarkko Sakkinen

use core::cmp;
use rstest::rstest;
use zmodem2::{
    read_subpacket, read_zpad, receive, write_subpacket, Buffer, Encoding, Error, Frame, Header,
    Packet, Read, State, Write, XON, ZDLE, ZPAD,
};

struct MockPort<'a> {
    input: &'a [u8],
    output: Vec<u8>,
}

impl<'a> MockPort<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            output: Vec::new(),
        }
    }
}

impl<'a> Read for MockPort<'a> {
    fn read(&mut self, buf: &mut [u8]) -> Result<u32, Error> {
        let n = cmp::min(self.input.len(), buf.len());
        buf[..n].copy_from_slice(&self.input[..n]);
        self.input = &self.input[n..];
        Ok(n as u32)
    }

    fn read_byte(&mut self) -> Result<u8, Error> {
        if let Some((&first, rest)) = self.input.split_first() {
            self.input = rest;
            Ok(first)
        } else {
            Err(Error::Read)
        }
    }
}

impl<'a> Write for MockPort<'a> {
    fn write_all(&mut self, buf: &[u8]) -> Result<(), Error> {
        self.output.extend_from_slice(buf);
        Ok(())
    }

    fn write_byte(&mut self, value: u8) -> Result<(), Error> {
        self.output.push(value);
        Ok(())
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
    assert!(header.write(&mut port) == Ok(()));
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
    assert!(Header::read(port) == Ok(Header::new(encoding, frame, flags)));
}

#[rstest]
#[case(Encoding::ZBIN, Packet::ZCRCE, &[])]
#[case(Encoding::ZBIN, Packet::ZCRCW, &[0x00])]
#[case(Encoding::ZBIN32, Packet::ZCRCQ, &[0, 1, 2, 3, 4, 0x60, 0x60])]
pub fn test_subpacket_read_write(
    #[case] encoding: Encoding,
    #[case] packet: Packet,
    #[case] data: &[u8],
) {
    let mut buf = Buffer::new();
    let mut port = vec![];
    assert!(write_subpacket(&mut port, encoding, packet, data) == Ok(()));
    buf.clear();
    assert!(read_subpacket(&mut port.as_slice(), &mut buf, encoding) == Ok(packet));
    assert!(buf == data);
}

#[rstest]
#[case(&[ZPAD, ZDLE], Ok(()))]
#[case(&[ZPAD, ZPAD, ZDLE], Ok(()))]
#[case(&[ZDLE], Err(Error::Data))]
#[case(&[ZPAD, XON], Err(Error::Data))]
#[case(&[ZPAD, ZPAD, XON], Err(Error::Data))]
#[case(&[], Err(Error::Read))]
#[case(&[0; 100], Err(Error::Data))]
pub fn test_zpad_read(#[case] port: &[u8], #[case] expected: Result<(), Error>) {
    assert!(read_zpad(&mut port.to_vec().as_slice()) == expected);
}

#[test]
fn test_receive_malformed_header() {
    let mut mock_port = MockPort::new(b"malformed data");
    let mut file = vec![];
    let mut state = State::new();

    let result = receive(&mut mock_port, &mut file, &mut state);

    assert!(result.is_ok());
}
