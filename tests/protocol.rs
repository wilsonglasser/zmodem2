// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2026 Jarkko Sakkinen

use rstest::rstest;
use zmodem2::{
    Effect, Encoding, Error, FileInfo, Frame, Header, Input, Progress, Receiver, ReceiverEvent,
    Sender, SenderEvent, SessionEvent, SubpacketType, XON, ZDLE, ZPAD,
};

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
    let mut receiver = Receiver::new().unwrap();
    receiver.advance_outgoing(receiver.drain_outgoing().len());

    let input = b"malformed data";
    let consumed = receiver.feed_incoming(input).unwrap();

    assert_eq!(consumed, input.len());
    assert!(receiver.drain_outgoing().is_empty());
    assert!(receiver.drain_file().is_empty());
    assert!(receiver.poll_event().is_none());
}

#[test]
fn test_receive_zfile_with_non_utf8_name() {
    let file_name = b"bad\x80name";
    let file_size = 123;

    let mut sender = Sender::new().unwrap();
    sender.start_file(file_name, file_size).unwrap();
    sender.advance_outgoing(sender.drain_outgoing().len());

    let zrinit = Header::new(Encoding::ZHEX, Frame::ZRINIT, &[0; 4]);
    let mut zrinit_bytes = Vec::new();
    zrinit.write(&mut zrinit_bytes).unwrap();
    let consumed = sender.feed_incoming(&zrinit_bytes).unwrap();
    assert!(consumed > 0 && consumed <= zrinit_bytes.len());

    let wire = sender.drain_outgoing().to_vec();
    sender.advance_outgoing(wire.len());

    let mut receiver = Receiver::new().unwrap();
    receiver.advance_outgoing(receiver.drain_outgoing().len());

    let mut input = wire.as_slice();
    let mut got_start = false;
    for _ in 0..(wire.len() * 4) {
        if input.is_empty() {
            break;
        }
        let consumed = receiver.feed_incoming(input).unwrap();
        if consumed == 0 {
            if !receiver.drain_outgoing().is_empty() {
                receiver.advance_outgoing(receiver.drain_outgoing().len());
            }
        } else {
            input = &input[consumed..];
        }

        if let Some(ReceiverEvent::FileStart) = receiver.poll_event() {
            got_start = true;
            break;
        }
    }

    assert!(got_start);
    assert_eq!(receiver.file_name(), file_name);
    assert_eq!(receiver.file_size(), file_size);
}

fn header_bytes(header: Header) -> Vec<u8> {
    let mut bytes = Vec::new();
    header.write(&mut bytes).unwrap();
    bytes
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

fn feed_receiver_zfile(receiver: &mut Receiver) {
    receiver.advance_outgoing(receiver.drain_outgoing().len());

    let mut bytes = header_bytes(Header::new(Encoding::ZBIN32, Frame::ZFILE, &[0; 4]));
    bytes.extend(escaped_zbin32_subpacket(
        SubpacketType::ZCRCW,
        b"file.bin\x00123\x00",
    ));

    let mut offset = 0;
    while offset < bytes.len() {
        let consumed = receiver.feed_incoming(&bytes[offset..]).unwrap();
        assert!(consumed > 0);
        offset += consumed;
        receiver.advance_outgoing(receiver.drain_outgoing().len());
    }
    assert_eq!(receiver.poll_event(), Some(ReceiverEvent::FileStart));
    receiver.advance_outgoing(receiver.drain_outgoing().len());
}

#[test]
fn test_sender_resumes_from_zrpos() {
    let mut sender = Sender::new().unwrap();
    sender.advance_outgoing(sender.drain_outgoing().len());
    sender.start_file(b"resume.bin", 16).unwrap();

    let zrinit = header_bytes(Header::new(Encoding::ZHEX, Frame::ZRINIT, &[0; 4]));
    assert!(sender.feed_incoming(&zrinit).unwrap() > 0);
    sender.advance_outgoing(sender.drain_outgoing().len());

    let zrpos = header_bytes(Header::new(
        Encoding::ZHEX,
        Frame::ZRPOS,
        &5u32.to_le_bytes(),
    ));
    assert!(sender.feed_incoming(&zrpos).unwrap() > 0);

    assert_eq!(sender.poll_file().unwrap().offset, 5);
}

#[test]
fn test_sender_skips_file_on_zskip() {
    let mut sender = Sender::new().unwrap();
    sender.advance_outgoing(sender.drain_outgoing().len());
    sender.start_file(b"skip.bin", 16).unwrap();

    let zrinit = header_bytes(Header::new(Encoding::ZHEX, Frame::ZRINIT, &[0; 4]));
    sender.feed_incoming(&zrinit).unwrap();
    sender.advance_outgoing(sender.drain_outgoing().len());

    let zskip = header_bytes(Header::new(Encoding::ZHEX, Frame::ZSKIP, &[0; 4]));
    sender.feed_incoming(&zskip).unwrap();

    assert_eq!(sender.poll_event(), Some(SenderEvent::FileComplete));
}

#[test]
fn test_abort_events() {
    let zabort = header_bytes(Header::new(Encoding::ZHEX, Frame::ZABORT, &[0; 4]));

    let mut sender = Sender::new().unwrap();
    sender.advance_outgoing(sender.drain_outgoing().len());
    assert!(sender.feed_incoming(&zabort).unwrap() > 0);
    assert_eq!(sender.poll_event(), Some(SenderEvent::Aborted));

    let mut receiver = Receiver::new().unwrap();
    receiver.advance_outgoing(receiver.drain_outgoing().len());
    assert!(receiver.feed_incoming(&zabort).unwrap() > 0);
    assert_eq!(receiver.poll_event(), Some(ReceiverEvent::Aborted));
}

#[test]
fn test_receiver_timeout_requeues_zrinit() {
    let mut receiver = Receiver::new().unwrap();
    receiver.advance_outgoing(receiver.drain_outgoing().len());

    match receiver.step(Input::Timeout).unwrap() {
        Progress::Effect(Effect::WriteWire(bytes)) => assert!(!bytes.is_empty()),
        progress => panic!("unexpected progress: {progress:?}"),
    }
}

#[test]
fn test_step_start_file_rejects_unknown_size() {
    let mut sender = Sender::new().unwrap();
    let info = FileInfo::new(b"unknown.bin", None);

    assert_eq!(
        sender.step(Input::StartFile(info)),
        Err(Error::UnsupportedFeature)
    );
}

#[test]
fn test_malformed_subpacket_crc() {
    let mut receiver = Receiver::new().unwrap();
    feed_receiver_zfile(&mut receiver);

    let mut bytes = header_bytes(Header::new(Encoding::ZBIN32, Frame::ZDATA, &[0; 4]));
    let mut subpacket = escaped_zbin32_subpacket(SubpacketType::ZCRCE, b"bad");
    let last = subpacket.last_mut().unwrap();
    *last ^= 0x01;
    bytes.extend(subpacket);

    let mut offset = 0;
    let err = loop {
        match receiver.feed_incoming(&bytes[offset..]) {
            Ok(consumed) => {
                assert!(consumed > 0);
                offset += consumed;
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

    let mut first = header_bytes(Header::new(Encoding::ZBIN32, Frame::ZDATA, &[0; 4]));
    first.extend(escaped_zbin32_subpacket(SubpacketType::ZCRCQ, b"abc"));
    let mut offset = 0;
    while offset < first.len() && receiver.drain_file().is_empty() {
        let consumed = receiver.feed_incoming(&first[offset..]).unwrap();
        assert!(consumed > 0);
        offset += consumed;
    }
    assert_eq!(receiver.drain_file(), b"abc");
    receiver.advance_file(3).unwrap();
    assert!(!receiver.drain_outgoing().is_empty());
    receiver.advance_outgoing(receiver.drain_outgoing().len());

    let second = escaped_zbin32_subpacket(SubpacketType::ZCRCE, b"def");
    let mut offset = 0;
    while offset < second.len() && receiver.drain_file().is_empty() {
        let consumed = receiver.feed_incoming(&second[offset..]).unwrap();
        assert!(consumed > 0);
        offset += consumed;
    }
    assert_eq!(receiver.drain_file(), b"def");
    receiver.advance_file(3).unwrap();
    assert!(receiver.drain_outgoing().is_empty());

    let zeof = header_bytes(Header::new(
        Encoding::ZBIN32,
        Frame::ZEOF,
        &6u32.to_le_bytes(),
    ));
    assert!(receiver.feed_incoming(&zeof).unwrap() > 0);
    assert_eq!(receiver.poll_event(), Some(ReceiverEvent::FileComplete));
}

#[test]
fn test_step_abort_event() {
    let mut sender = Sender::new().unwrap();
    sender.advance_outgoing(sender.drain_outgoing().len());

    match sender.step(Input::Abort).unwrap() {
        Progress::Effect(Effect::Event(SessionEvent::Aborted)) => {}
        progress => panic!("unexpected progress: {progress:?}"),
    }
}
