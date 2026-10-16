//! UDP control packets (DS -> roboRIO, port 1110) and status packets (roboRIO -> DS, port 1150).

use bitflags::bitflags;
use bytes::{Buf, BufMut};

use crate::Mode;

/// Bits of the mode field shared by the control and status bytes.
const MODE_MASK: u8 = 0b11;

bitflags! {
    /// The control byte of an outbound packet
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    pub struct Control: u8 {
        const ESTOP = 0b1000_0000;
        const FMS_CONNECTED = 0b0000_1000;
        const ENABLED = 0b0000_0100;
        const TEST = 0b01;
        const AUTO = 0b10;
    }
}

bitflags! {
    /// Requests for the roboRIO to reboot or restart user code
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    pub struct Request: u8 {
        const REBOOT_ROBORIO = 0b0000_1000;
        const RESTART_CODE = 0b0000_0100;
    }
}

bitflags! {
    /// The status byte of an inbound packet
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    pub struct Status: u8 {
        const ESTOP = 0b1000_0000;
        const BROWNOUT = 0b0001_0000;
        const CODE_START = 0b0000_1000;
        const ENABLED = 0b0000_0100;
        const TEST = 0b01;
        const AUTO = 0b10;
    }
}

bitflags! {
    /// The trace byte of an inbound packet, describing what the robot is currently doing
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Default)]
    pub struct Trace: u8 {
        const ROBOT_CODE = 0b0010_0000;
        const IS_ROBORIO = 0b0001_0000;
        const TEST_MODE = 0b0000_1000;
        const AUTONOMOUS = 0b0000_0100;
        const TELEOP = 0b0000_0010;
        const DISABLED = 0b0000_0001;
    }
}

impl Trace {
    pub fn is_code_started(self) -> bool {
        self.contains(Trace::ROBOT_CODE)
    }

    pub fn is_roborio(self) -> bool {
        self.contains(Trace::IS_ROBORIO)
    }

    pub fn is_disabled(self) -> bool {
        self.contains(Trace::DISABLED)
    }
}

impl Status {
    pub fn is_browning_out(self) -> bool {
        self.contains(Status::BROWNOUT)
    }

    pub fn emergency_stopped(self) -> bool {
        self.contains(Status::ESTOP)
    }

    /// The mode the robot reports it is in
    pub fn mode(self) -> Option<Mode> {
        match self.bits() & MODE_MASK {
            0b00 => Some(Mode::Teleoperated),
            0b01 => Some(Mode::Test),
            0b10 => Some(Mode::Autonomous),
            _ => None,
        }
    }
}

impl Mode {
    pub(crate) fn to_control(self) -> Control {
        match self {
            Mode::Teleoperated => Control::empty(),
            Mode::Test => Control::TEST,
            Mode::Autonomous => Control::AUTO,
        }
    }
}

/// Alliance colour and station, encoded as the byte the roboRIO expects
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Alliance(pub u8);

impl Alliance {
    /// Red alliance, station `position` (1-3)
    pub fn new_red(position: u8) -> Alliance {
        Alliance(position.clamp(1, 3) - 1)
    }

    /// Blue alliance, station `position` (1-3)
    pub fn new_blue(position: u8) -> Alliance {
        Alliance(position.clamp(1, 3) + 2)
    }

    pub fn is_red(self) -> bool {
        self.0 < 3
    }

    pub fn is_blue(self) -> bool {
        !self.is_red()
    }

    /// The station number, 1-3
    pub fn position(self) -> u8 {
        (self.0 % 3) + 1
    }
}

/// Tags that can be appended to an outbound control packet
#[derive(Clone, Debug, PartialEq)]
pub enum UdpTag {
    /// Time left in the current match period
    Countdown(f32),
    /// Joystick values for one joystick slot
    Joystick(JoystickTag),
    /// Current UTC time, sent when the roboRIO asks for it
    DateTime(chrono::DateTime<chrono::Utc>),
    /// Timezone name, sent alongside DateTime
    Timezone(String),
}

/// Values for one joystick slot. An empty tag tells robot code the slot is unplugged.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JoystickTag {
    pub axes: Vec<i8>,
    pub buttons: Vec<bool>,
    pub povs: Vec<i16>,
}

impl UdpTag {
    fn id(&self) -> u8 {
        match self {
            UdpTag::Countdown(_) => 0x07,
            UdpTag::Joystick(_) => 0x0c,
            UdpTag::DateTime(_) => 0x0f,
            UdpTag::Timezone(_) => 0x10,
        }
    }

    fn data(&self, buf: &mut Vec<u8>) {
        match self {
            UdpTag::Countdown(secs) => buf.put_f32(*secs),
            UdpTag::Joystick(js) => {
                buf.put_u8(js.axes.len() as u8);
                for axis in &js.axes {
                    buf.put_i8(*axis);
                }

                // Buttons are a bitfield (button n at bit n) written big endian
                buf.put_u8(js.buttons.len() as u8);
                let bytes = js.buttons.len().div_ceil(8);
                let bits = js
                    .buttons
                    .iter()
                    .enumerate()
                    .fold(0u64, |acc, (i, &pressed)| acc | ((pressed as u64) << i));
                buf.extend_from_slice(&bits.to_be_bytes()[8 - bytes..]);

                buf.put_u8(js.povs.len() as u8);
                for pov in &js.povs {
                    buf.put_i16(*pov);
                }
            }
            UdpTag::DateTime(time) => {
                use chrono::{Datelike, Timelike};
                buf.put_u32(time.timestamp_subsec_micros());
                buf.put_u8(time.second() as u8);
                buf.put_u8(time.minute() as u8);
                buf.put_u8(time.hour() as u8);
                buf.put_u8(time.day() as u8);
                buf.put_u8(time.month0() as u8);
                buf.put_u8((time.year() - 1900) as u8);
            }
            UdpTag::Timezone(tz) => buf.extend_from_slice(tz.as_bytes()),
        }
    }

    /// Encodes `[size, id, data...]`
    pub(crate) fn encode(&self, buf: &mut Vec<u8>) {
        let start = buf.len();
        buf.put_u8(0); // size placeholder
        buf.put_u8(self.id());
        self.data(buf);
        buf[start] = (buf.len() - start - 1) as u8;
    }
}

/// UDP control packet sent to the roboRIO every 20ms
#[derive(Debug, Clone, PartialEq)]
pub struct ControlPacket {
    pub seqnum: u16,
    pub control: Control,
    pub request: Request,
    pub alliance: Alliance,
    pub tags: Vec<UdpTag>,
}

impl ControlPacket {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64);
        buf.put_u16(self.seqnum);
        buf.put_u8(0x01); // comm version
        buf.put_u8(self.control.bits());
        buf.put_u8(self.request.bits());
        buf.put_u8(self.alliance.0);
        for tag in &self.tags {
            tag.encode(&mut buf);
        }
        buf
    }
}

impl ControlPacket {
    /// Decodes a control packet, used to fake a roboRIO in tests. Only joystick tags are kept.
    pub fn decode(mut buf: &[u8]) -> Option<ControlPacket> {
        if buf.remaining() < 6 {
            return None;
        }
        let seqnum = buf.get_u16();
        let _comm_version = buf.get_u8();
        let control = Control::from_bits_retain(buf.get_u8());
        let request = Request::from_bits_retain(buf.get_u8());
        let alliance = Alliance(buf.get_u8());
        let mut tags = Vec::new();
        while buf.remaining() >= 2 {
            let size = buf.get_u8() as usize;
            if size == 0 || buf.remaining() < size {
                return None;
            }
            let mut tag = &buf[..size];
            buf.advance(size);
            if tag.get_u8() != 0x0c {
                continue;
            }
            let mut js = JoystickTag::default();
            let axes = tag.get_u8() as usize;
            js.axes = (0..axes).map(|_| tag.get_i8()).collect();
            let buttons = tag.get_u8() as usize;
            let mut bits = 0u64;
            for _ in 0..buttons.div_ceil(8) {
                bits = (bits << 8) | u64::from(tag.get_u8());
            }
            js.buttons = (0..buttons).map(|i| bits & (1 << i) != 0).collect();
            let povs = tag.get_u8() as usize;
            js.povs = (0..povs).map(|_| tag.get_i16()).collect();
            tags.push(UdpTag::Joystick(js));
        }
        Some(ControlPacket {
            seqnum,
            control,
            request,
            alliance,
            tags,
        })
    }

    pub fn enabled(&self) -> bool {
        self.control.contains(Control::ENABLED)
    }

    pub fn estopped(&self) -> bool {
        self.control.contains(Control::ESTOP)
    }

    pub fn mode(&self) -> Option<Mode> {
        Status::from_bits_retain(self.control.bits()).mode()
    }
}

/// UDP status packet sent by the roboRIO in response to each control packet
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatusPacket {
    pub seqnum: u16,
    pub status: Status,
    pub trace: Trace,
    pub battery: f32,
    pub need_date: bool,
}

impl StatusPacket {
    /// Decodes the fixed header of a status packet. Trailing tags (CPU, RAM, CAN usage...) are ignored.
    pub fn decode(mut buf: &[u8]) -> Option<StatusPacket> {
        if buf.remaining() < 8 {
            return None;
        }
        let seqnum = buf.get_u16();
        let _comm_version = buf.get_u8();
        let status = Status::from_bits_retain(buf.get_u8());
        let trace = Trace::from_bits_retain(buf.get_u8());
        let battery = f32::from(buf.get_u8()) + f32::from(buf.get_u8()) / 256.0;
        let need_date = buf.get_u8() == 1;
        Some(StatusPacket {
            seqnum,
            status,
            trace,
            battery,
            need_date,
        })
    }

    /// Encodes a status packet, used to fake a roboRIO in tests.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8);
        buf.put_u16(self.seqnum);
        buf.put_u8(0x01);
        buf.put_u8(self.status.bits());
        buf.put_u8(self.trace.bits());
        let whole = self.battery.trunc();
        buf.put_u8(whole as u8);
        buf.put_u8(((self.battery - whole) * 256.0) as u8);
        buf.put_u8(self.need_date as u8);
        buf
    }
}

/// Converts an axis value in `-1.0..=1.0` to the signed byte the roboRIO expects
pub fn encode_axis(value: f32) -> i8 {
    let value = if value.is_nan() {
        0.0
    } else {
        value.clamp(-1.0, 1.0)
    };
    if value >= 1.0 {
        127
    } else {
        (value * 128.0) as i8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_tag_format() {
        let mut buf = vec![];
        UdpTag::Countdown(2.0).encode(&mut buf);
        assert_eq!(buf, [0x05, 0x07, 0x40, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn joystick_buttons_are_big_endian_bitfield() {
        let mut buttons = vec![false; 10];
        buttons[0] = true;
        buttons[9] = true;
        let mut buf = vec![];
        UdpTag::Joystick(JoystickTag {
            axes: vec![0, 127],
            buttons,
            povs: vec![-1],
        })
        .encode(&mut buf);
        assert_eq!(
            buf,
            [
                10,
                0x0c,
                2,
                0,
                127,
                10,
                0b0000_0010,
                0b0000_0001,
                1,
                0xff,
                0xff
            ]
        );
    }

    #[test]
    fn unplugged_joystick() {
        let mut buf = vec![];
        UdpTag::Joystick(JoystickTag::default()).encode(&mut buf);
        assert_eq!(buf, [4, 0x0c, 0, 0, 0]);
    }

    #[test]
    fn control_packet_header() {
        let packet = ControlPacket {
            seqnum: 0x0102,
            control: Mode::Autonomous.to_control() | Control::ENABLED,
            request: Request::RESTART_CODE,
            alliance: Alliance::new_blue(2),
            tags: vec![],
        };
        assert_eq!(packet.encode(), [0x01, 0x02, 0x01, 0b0110, 0b0100, 4]);
        assert_eq!(ControlPacket::decode(&packet.encode()), Some(packet));
    }

    #[test]
    fn joystick_tags_round_trip() {
        let mut buttons = vec![false; 10];
        buttons[3] = true;
        let packet = ControlPacket {
            seqnum: 1,
            control: Control::ENABLED,
            request: Request::empty(),
            alliance: Alliance::new_red(1),
            tags: vec![
                UdpTag::Joystick(JoystickTag::default()),
                UdpTag::Joystick(JoystickTag {
                    axes: vec![1, -2, 3, 0, 0, 127],
                    buttons,
                    povs: vec![270],
                }),
            ],
        };
        let decoded = ControlPacket::decode(&packet.encode()).unwrap();
        assert_eq!(decoded, packet);
        assert_eq!(decoded.mode(), Some(Mode::Teleoperated));
    }

    #[test]
    fn status_round_trip() {
        let packet = StatusPacket {
            seqnum: 7,
            status: Status::CODE_START | Status::AUTO,
            trace: Trace::ROBOT_CODE | Trace::IS_ROBORIO,
            battery: 12.5,
            need_date: true,
        };
        assert_eq!(StatusPacket::decode(&packet.encode()), Some(packet));
        assert_eq!(packet.status.mode(), Some(Mode::Autonomous));
    }

    #[test]
    fn axis_encoding() {
        assert_eq!(encode_axis(1.0), 127);
        assert_eq!(encode_axis(-1.0), -128);
        assert_eq!(encode_axis(0.0), 0);
        assert_eq!(encode_axis(5.0), 127);
        assert_eq!(encode_axis(f32::NAN), 0);
    }

    #[test]
    fn alliance_positions() {
        assert!(Alliance::new_red(3).is_red());
        assert_eq!(Alliance::new_blue(1).0, 3);
        assert_eq!(Alliance::new_blue(3).position(), 3);
    }
}
