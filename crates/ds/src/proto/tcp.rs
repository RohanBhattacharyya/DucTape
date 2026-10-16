//! TCP traffic on port 1740. Frames are `[u16 size][u8 id][size - 1 bytes]`.

use bytes::{Buf, BufMut, BytesMut};

/// Messages received from the roboRIO
#[derive(Debug, Clone, PartialEq)]
pub enum TcpPacket {
    /// A line printed to standard output by robot code
    Stdout(Stdout),
    /// An error or warning reported through the WPILib error API
    Error(ErrorMessage),
    /// A frame this library doesn't decode
    Other { id: u8 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stdout {
    pub timestamp: f32,
    pub seqnum: u16,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ErrorMessage {
    pub timestamp: f32,
    pub seqnum: u16,
    pub code: i32,
    /// `true` for errors, `false` for warnings
    pub is_error: bool,
    pub details: String,
    pub location: String,
    pub call_stack: String,
}

/// Messages sent to the roboRIO
#[derive(Debug, Clone, PartialEq)]
pub enum TcpTag {
    /// The Game Specific Message available to robot code
    GameData(String),
}

impl TcpTag {
    pub fn encode(&self) -> Vec<u8> {
        let (id, data) = match self {
            TcpTag::GameData(gsm) => (0x0e, gsm.as_bytes()),
        };
        let mut buf = Vec::with_capacity(data.len() + 3);
        buf.put_u16(data.len() as u16 + 1);
        buf.put_u8(id);
        buf.extend_from_slice(data);
        buf
    }
}

/// Pops one complete frame from `buf`, returning `None` if more data is needed.
pub fn decode_frame(buf: &mut BytesMut) -> Option<TcpPacket> {
    if buf.len() < 2 {
        return None;
    }
    let size = u16::from_be_bytes([buf[0], buf[1]]) as usize;
    if buf.len() < size + 2 {
        return None;
    }
    buf.advance(2);
    let frame = buf.split_to(size).freeze();
    if frame.is_empty() {
        return Some(TcpPacket::Other { id: 0 });
    }
    let id = frame[0];
    let body = &frame[1..];
    Some(
        match id {
            0x0c => decode_stdout(body).map(TcpPacket::Stdout),
            0x0b => decode_error(body).map(TcpPacket::Error),
            _ => None,
        }
        .unwrap_or(TcpPacket::Other { id }),
    )
}

fn decode_stdout(mut body: &[u8]) -> Option<Stdout> {
    if body.remaining() < 6 {
        return None;
    }
    let timestamp = body.get_f32();
    let seqnum = body.get_u16();
    Some(Stdout {
        timestamp,
        seqnum,
        message: String::from_utf8_lossy(body).into_owned(),
    })
}

fn decode_error(mut body: &[u8]) -> Option<ErrorMessage> {
    fn string(body: &mut &[u8]) -> Option<String> {
        if body.remaining() < 2 {
            return None;
        }
        let len = body.get_u16() as usize;
        if body.remaining() < len {
            return None;
        }
        let s = String::from_utf8_lossy(&body[..len]).into_owned();
        body.advance(len);
        Some(s)
    }

    if body.remaining() < 13 {
        return None;
    }
    let timestamp = body.get_f32();
    let seqnum = body.get_u16();
    let _occurrences = body.get_u16();
    let code = body.get_i32();
    let flags = body.get_u8();
    let details = string(&mut body)?;
    let location = string(&mut body)?;
    let call_stack = string(&mut body)?;
    Some(ErrorMessage {
        timestamp,
        seqnum,
        code,
        is_error: flags & 0x01 != 0,
        details,
        location,
        call_stack,
    })
}

/// Encodes a stdout frame, used to fake a roboRIO in tests.
pub fn encode_stdout(stdout: &Stdout) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.put_u16(stdout.message.len() as u16 + 7);
    buf.put_u8(0x0c);
    buf.put_f32(stdout.timestamp);
    buf.put_u16(stdout.seqnum);
    buf.extend_from_slice(stdout.message.as_bytes());
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdout_round_trip_and_partial_frames() {
        let stdout = Stdout {
            timestamp: 1.5,
            seqnum: 3,
            message: "Robot program starting".into(),
        };
        let bytes = encode_stdout(&stdout);
        let mut buf = BytesMut::from(&bytes[..4]);
        assert_eq!(decode_frame(&mut buf), None);
        buf.extend_from_slice(&bytes[4..]);
        buf.extend_from_slice(&bytes);
        assert_eq!(
            decode_frame(&mut buf),
            Some(TcpPacket::Stdout(stdout.clone()))
        );
        assert_eq!(decode_frame(&mut buf), Some(TcpPacket::Stdout(stdout)));
        assert!(buf.is_empty());
    }

    #[test]
    fn error_message() {
        let mut body = vec![0x0b];
        body.put_f32(2.0);
        body.put_u16(9);
        body.put_u16(1);
        body.put_i32(-44);
        body.put_u8(1);
        for s in ["Joystick Button missing", "Joystick.cpp", ""] {
            body.put_u16(s.len() as u16);
            body.extend_from_slice(s.as_bytes());
        }
        let mut buf = BytesMut::new();
        buf.put_u16(body.len() as u16);
        buf.extend_from_slice(&body);
        let Some(TcpPacket::Error(err)) = decode_frame(&mut buf) else {
            panic!("expected error frame");
        };
        assert!(err.is_error);
        assert_eq!(err.code, -44);
        assert_eq!(err.details, "Joystick Button missing");
        assert_eq!(err.location, "Joystick.cpp");
    }

    #[test]
    fn game_data() {
        assert_eq!(
            TcpTag::GameData("LRL".into()).encode(),
            [0, 4, 0x0e, b'L', b'R', b'L']
        );
    }

    #[test]
    fn unknown_frames_are_skipped() {
        let mut buf = BytesMut::from(&[0u8, 2, 0x0a, 0xff][..]);
        assert_eq!(decode_frame(&mut buf), Some(TcpPacket::Other { id: 0x0a }));
        assert!(buf.is_empty());
    }
}
