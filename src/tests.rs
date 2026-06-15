// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2026 Jarkko Sakkinen

//! Protocol-level unit tests exercising crate-internal framing and the
//! public poll/submit API.

use crate::buffer::Buffer;
use crate::header::{Encoding, Frame, Header};
use crate::wire::{BufferWriter, HeaderReader, SliceReader, SubpacketType};
use crate::{Action, Error, Event, FileInfo, Position, Receiver, Sender, ZDLE, ZPAD};
use rstest::rstest;

fn write_header(header: Header) -> Vec<u8> {
    let mut buf = Buffer::<64>::new();
    let mut writer = BufferWriter::new(&mut buf);
    assert_eq!(header.write(&mut writer), Ok(Some(())));
    buf.to_vec()
}

/// Parses a header through the production [`HeaderReader`], prepending the
/// `ZPAD ZDLE` framing the reader seeks for.
fn read_header(bytes: &[u8]) -> Header {
    let mut framed = Vec::with_capacity(bytes.len() + 2);
    framed.push(ZPAD);
    framed.push(ZDLE);
    framed.extend_from_slice(bytes);
    let mut reader = SliceReader::new(&framed);
    let mut header_reader = HeaderReader::new();
    header_reader.read(&mut reader).unwrap().unwrap()
}

/// Drains pending outgoing wire bytes while no event is queued.
fn drain_wire_sender(sender: &mut Sender) {
    while let Action::WriteWire(bytes) = sender.poll() {
        let n = bytes.len();
        sender.wire_written(n);
    }
}

fn drain_wire_receiver(receiver: &mut Receiver) {
    while let Action::WriteWire(bytes) = receiver.poll() {
        let n = bytes.len();
        receiver.wire_written(n);
    }
}

#[rstest]
#[case(Encoding::ZBIN, Frame::ZRQINIT, [0; 4], &[ZPAD, ZDLE, Encoding::ZBIN as u8, 0, 0, 0, 0, 0, 0, 0])]
#[case(Encoding::ZBIN32, Frame::ZRQINIT, [0; 4], &[ZPAD, ZDLE, Encoding::ZBIN32 as u8, 0, 0, 0, 0, 0, 29, 247, 34, 198])]
#[case(Encoding::ZBIN, Frame::ZRQINIT, [1; 4], &[ZPAD, ZDLE, Encoding::ZBIN as u8, 0, 1, 1, 1, 1, 98, 148])]
#[case(Encoding::ZHEX, Frame::ZRQINIT, [1; 4], &[ZPAD, ZPAD, ZDLE, Encoding::ZHEX as u8, b'0', b'0', b'0', b'1', b'0', b'1', b'0', b'1', b'0', b'1', 54, 50, 57, 52, b'\r', b'\n', crate::XON])]
fn test_header_write(
    #[case] encoding: Encoding,
    #[case] frame: Frame,
    #[case] flags: [u8; 4],
    #[case] expected: &[u8],
) {
    assert_eq!(write_header(Header::new(encoding, frame, flags)), expected);
}

#[rstest]
#[case(&[Encoding::ZHEX as u8, b'0', b'1', b'0', b'1', b'0', b'2', b'0', b'3', b'0', b'4', b'a', b'7', b'5', b'2'], Encoding::ZHEX, Frame::ZRINIT, [0x1, 0x2, 0x3, 0x4])]
#[case(&[Encoding::ZBIN as u8, Frame::ZRINIT as u8, 0xa, 0xb, 0xc, 0xd, 0xa6, 0xcb], Encoding::ZBIN, Frame::ZRINIT, [0xa, 0xb, 0xc, 0xd])]
#[case(&[Encoding::ZBIN32 as u8, Frame::ZRINIT as u8, 0xa, 0xb, 0xc, 0xd, 0x99, 0xe2, 0xae, 0x4a], Encoding::ZBIN32, Frame::ZRINIT, [0xa, 0xb, 0xc, 0xd])]
#[case(&[Encoding::ZBIN as u8, Frame::ZRINIT as u8, 0xa, ZDLE, b'l', 0xd, ZDLE, b'm', 0x5e, 0x6f], Encoding::ZBIN, Frame::ZRINIT, [0xa, 0x7f, 0xd, 0xff])]
fn test_header_read(
    #[case] port: &[u8],
    #[case] encoding: Encoding,
    #[case] frame: Frame,
    #[case] flags: [u8; 4],
) {
    assert_eq!(read_header(port), Header::new(encoding, frame, flags));
}

#[test]
fn test_receive_malformed_header() {
    let mut receiver = Receiver::new().unwrap();
    drain_wire_receiver(&mut receiver);

    let input = b"malformed data";
    let consumed = receiver.submit_wire(input).unwrap();

    assert_eq!(consumed, input.len());
    assert_eq!(receiver.poll(), Action::Idle);
}

#[test]
fn test_receive_zfile_with_non_utf8_name() {
    let file_name = b"bad\x80name";
    let file_size = 123u32;

    let mut sender = Sender::new().unwrap();
    drain_wire_sender(&mut sender);
    sender
        .start_file(FileInfo::new(file_name, Some(Position::new(file_size))))
        .unwrap();

    let zrinit = write_header(Header::new(Encoding::ZHEX, Frame::ZRINIT, [0; 4]));
    assert!(sender.submit_wire(&zrinit).unwrap() > 0);

    // Collect the ZFILE frame the sender emits.
    let mut wire = Vec::new();
    while let Action::WriteWire(bytes) = sender.poll() {
        wire.extend_from_slice(bytes);
        let n = bytes.len();
        sender.wire_written(n);
    }
    assert!(!wire.is_empty());

    let mut receiver = Receiver::new().unwrap();
    drain_wire_receiver(&mut receiver);

    let mut offset = 0;
    let mut started: Option<(Vec<u8>, Option<u32>)> = None;
    while started.is_none() {
        match receiver.poll() {
            Action::WriteWire(bytes) => {
                let n = bytes.len();
                receiver.wire_written(n);
            }
            Action::Event(Event::FileStarted(info)) => {
                started = Some((info.name.to_vec(), info.size.map(Position::get)));
            }
            Action::Event(event) => panic!("unexpected event: {event:?}"),
            Action::Idle => {
                assert!(offset < wire.len(), "ran out of input before FileStarted");
                let consumed = receiver.submit_wire(&wire[offset..]).unwrap();
                assert!(consumed > 0);
                offset += consumed;
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }

    let (name, size) = started.unwrap();
    assert_eq!(name, file_name);
    assert_eq!(size, Some(file_size));
}

fn test_crc32_iso_hdlc(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xedb8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

fn encode_zdle(byte: u8) -> u8 {
    match byte {
        0x0d => 0x4d,
        0x10 => 0x50,
        0x11 => 0x51,
        0x13 => 0x53,
        0x18 => 0x58,
        0x7f => 0x6c,
        0x8d => 0xcd,
        0x90 => 0xd0,
        0x91 => 0xd1,
        0x93 => 0xd3,
        0xff => 0x6d,
        _ => byte,
    }
}

fn escaped_zbin32_subpacket(kind: SubpacketType, data: &[u8]) -> Vec<u8> {
    fn push_escaped(out: &mut Vec<u8>, byte: u8) {
        let escaped = encode_zdle(byte);
        if escaped != byte {
            out.push(ZDLE);
        }
        out.push(escaped);
    }

    let mut out = Vec::new();
    for &byte in data {
        push_escaped(&mut out, byte);
    }
    out.push(ZDLE);
    out.push(kind as u8);

    let mut crc_input = Vec::from(data);
    crc_input.push(kind as u8);
    for byte in test_crc32_iso_hdlc(&crc_input).to_le_bytes() {
        push_escaped(&mut out, byte);
    }
    out
}

/// Drives a receiver through a ZFILE frame, leaving it ready to receive data.
fn feed_receiver_zfile(receiver: &mut Receiver) {
    drain_wire_receiver(receiver);

    let mut bytes = write_header(Header::new(Encoding::ZBIN32, Frame::ZFILE, [0; 4]));
    bytes.extend(escaped_zbin32_subpacket(
        SubpacketType::ZCRCW,
        b"file.bin\x00123\x00",
    ));

    let mut offset = 0;
    let mut started = false;
    while !started {
        match receiver.poll() {
            Action::WriteWire(b) => {
                let n = b.len();
                receiver.wire_written(n);
            }
            Action::Event(Event::FileStarted(_)) => started = true,
            Action::Event(event) => panic!("unexpected event: {event:?}"),
            Action::Idle => {
                assert!(offset < bytes.len(), "ran out of input before FileStarted");
                let consumed = receiver.submit_wire(&bytes[offset..]).unwrap();
                assert!(consumed > 0);
                offset += consumed;
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }

    drain_wire_receiver(receiver);
}

#[test]
fn test_sender_resumes_from_zrpos() {
    let mut sender = Sender::new().unwrap();
    drain_wire_sender(&mut sender);
    sender
        .start_file(FileInfo::new(b"resume.bin", Some(Position::new(16))))
        .unwrap();

    let zrinit = write_header(Header::new(Encoding::ZHEX, Frame::ZRINIT, [0; 4]));
    assert!(sender.submit_wire(&zrinit).unwrap() > 0);
    drain_wire_sender(&mut sender);

    let zrpos = write_header(Header::new(
        Encoding::ZHEX,
        Frame::ZRPOS,
        5u32.to_le_bytes(),
    ));
    assert!(sender.submit_wire(&zrpos).unwrap() > 0);

    match sender.poll() {
        Action::ReadFile { offset, .. } => assert_eq!(offset, Position::new(5)),
        other => panic!("unexpected action: {other:?}"),
    }
}

#[test]
fn test_sender_skips_file_on_zskip() {
    let mut sender = Sender::new().unwrap();
    drain_wire_sender(&mut sender);
    sender
        .start_file(FileInfo::new(b"skip.bin", Some(Position::new(16))))
        .unwrap();

    let zrinit = write_header(Header::new(Encoding::ZHEX, Frame::ZRINIT, [0; 4]));
    sender.submit_wire(&zrinit).unwrap();
    drain_wire_sender(&mut sender);

    let zskip = write_header(Header::new(Encoding::ZHEX, Frame::ZSKIP, [0; 4]));
    sender.submit_wire(&zskip).unwrap();

    assert_eq!(sender.poll(), Action::Event(Event::FileCompleted));
}

#[test]
fn test_abort_events() {
    let zabort = write_header(Header::new(Encoding::ZHEX, Frame::ZABORT, [0; 4]));

    let mut sender = Sender::new().unwrap();
    drain_wire_sender(&mut sender);
    assert!(sender.submit_wire(&zabort).unwrap() > 0);
    assert_eq!(sender.poll(), Action::Event(Event::Aborted));

    let mut receiver = Receiver::new().unwrap();
    drain_wire_receiver(&mut receiver);
    assert!(receiver.submit_wire(&zabort).unwrap() > 0);
    assert_eq!(receiver.poll(), Action::Event(Event::Aborted));
}

#[test]
fn test_receiver_timeout_requeues_zrinit() {
    let mut receiver = Receiver::new().unwrap();
    drain_wire_receiver(&mut receiver);

    receiver.timeout().unwrap();

    match receiver.poll() {
        Action::WriteWire(bytes) => assert!(!bytes.is_empty()),
        other => panic!("unexpected action: {other:?}"),
    }
}

#[test]
fn test_start_file_rejects_unknown_size() {
    let mut sender = Sender::new().unwrap();

    assert_eq!(
        sender.start_file(FileInfo::new(b"unknown.bin", None)),
        Err(Error::UnsupportedFeature)
    );
}

#[test]
fn test_malformed_subpacket_crc() {
    let mut receiver = Receiver::new().unwrap();
    feed_receiver_zfile(&mut receiver);

    let mut bytes = write_header(Header::new(Encoding::ZBIN32, Frame::ZDATA, [0; 4]));
    let mut subpacket = escaped_zbin32_subpacket(SubpacketType::ZCRCE, b"bad");
    let last = subpacket.last_mut().unwrap();
    *last ^= 0x01;
    bytes.extend(subpacket);

    let mut offset = 0;
    let err = loop {
        match receiver.submit_wire(&bytes[offset..]) {
            Ok(consumed) => {
                offset += consumed;
                if consumed == 0 {
                    match receiver.poll() {
                        Action::WriteWire(b) => {
                            let n = b.len();
                            receiver.wire_written(n);
                        }
                        Action::WriteFile(b) => {
                            let n = b.len();
                            receiver.file_written(n).unwrap();
                        }
                        other => panic!("no progress: {other:?}"),
                    }
                }
            }
            Err(error) => break error,
        }
    };
    assert_eq!(err, Error::UnexpectedCrc32);
}

#[test]
fn test_receiver_zcrcq_and_zcrce() {
    let mut receiver = Receiver::new().unwrap();
    feed_receiver_zfile(&mut receiver);

    let mut first = write_header(Header::new(Encoding::ZBIN32, Frame::ZDATA, [0; 4]));
    first.extend(escaped_zbin32_subpacket(SubpacketType::ZCRCQ, b"abc"));
    consume_file_chunk(&mut receiver, &first, b"abc");

    // ZCRCQ requests an acknowledgement, so a ZACK is queued.
    match receiver.poll() {
        Action::WriteWire(bytes) => {
            assert!(!bytes.is_empty());
            let n = bytes.len();
            receiver.wire_written(n);
        }
        other => panic!("expected ZACK, got {other:?}"),
    }

    let second = escaped_zbin32_subpacket(SubpacketType::ZCRCE, b"def");
    consume_file_chunk(&mut receiver, &second, b"def");

    // ZCRCE ends the frame without an acknowledgement.
    assert_eq!(receiver.poll(), Action::Idle);

    let zeof = write_header(Header::new(
        Encoding::ZBIN32,
        Frame::ZEOF,
        6u32.to_le_bytes(),
    ));
    assert!(receiver.submit_wire(&zeof).unwrap() > 0);
    assert_eq!(receiver.poll(), Action::Event(Event::FileCompleted));
}

/// Feeds a single data subpacket and asserts the receiver yields `expected`
/// file bytes, then acknowledges them.
fn consume_file_chunk(receiver: &mut Receiver, input: &[u8], expected: &[u8]) {
    let mut offset = 0;
    loop {
        match receiver.poll() {
            Action::WriteFile(bytes) => {
                assert_eq!(bytes, expected);
                let n = bytes.len();
                receiver.file_written(n).unwrap();
                break;
            }
            Action::WriteWire(bytes) => {
                let n = bytes.len();
                receiver.wire_written(n);
            }
            Action::Idle => {
                assert!(offset < input.len(), "ran out of input before file data");
                let consumed = receiver.submit_wire(&input[offset..]).unwrap();
                offset += consumed;
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }
}

#[test]
fn test_abort_event() {
    let mut sender = Sender::new().unwrap();
    drain_wire_sender(&mut sender);

    sender.abort();
    assert_eq!(sender.poll(), Action::Event(Event::Aborted));
}
