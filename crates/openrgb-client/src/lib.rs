//! A small synchronous client for the OpenRGB network SDK.
//!
//! The implementation intentionally focuses on the stable controller API used
//! by OpenRGB protocol versions 1 through 5. It supports protocol negotiation,
//! controller discovery, matrix metadata, Direct mode, and full-device LED
//! updates without tying callers to an async runtime.

use std::{
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    time::Duration,
};

use thiserror::Error;

const MAGIC: [u8; 4] = *b"ORGB";
const HEADER_SIZE: usize = 16;
const MAX_PACKET_SIZE: usize = 8 * 1024 * 1024;

const REQUEST_CONTROLLER_COUNT: u32 = 0;
const REQUEST_CONTROLLER_DATA: u32 = 1;
const REQUEST_PROTOCOL_VERSION: u32 = 40;
const SET_CLIENT_NAME: u32 = 50;
const UPDATE_LEDS: u32 = 1050;
const SET_CUSTOM_MODE: u32 = 1100;

/// Highest stable OpenRGB protocol version supported by this client.
pub const MAX_SUPPORTED_PROTOCOL: u32 = 5;

/// Result type returned by the OpenRGB client.
pub type Result<T> = std::result::Result<T, OpenRgbError>;

/// Errors produced while connecting to or communicating with OpenRGB.
#[derive(Debug, Error)]
pub enum OpenRgbError {
    /// A network operation failed.
    #[error("OpenRGB network operation failed: {0}")]
    Io(#[from] std::io::Error),
    /// The server returned a malformed or unsupported packet.
    #[error("Invalid OpenRGB packet: {0}")]
    Protocol(String),
}

/// An RGB color in OpenRGB channel order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Color {
    /// Red channel.
    pub red: u8,
    /// Green channel.
    pub green: u8,
    /// Blue channel.
    pub blue: u8,
}

impl Color {
    /// Creates a color from red, green, and blue channels.
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    fn from_packed(value: u32) -> Self {
        Self {
            red: value as u8,
            green: (value >> 8) as u8,
            blue: (value >> 16) as u8,
        }
    }

    fn packed(self) -> u32 {
        u32::from(self.red) | (u32::from(self.green) << 8) | (u32::from(self.blue) << 16)
    }
}

/// OpenRGB controller device category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    /// A keyboard controller.
    Keyboard,
    /// Any controller category not currently exposed by this crate.
    Other(i32),
}

impl From<i32> for DeviceType {
    fn from(value: i32) -> Self {
        if value == 5 {
            Self::Keyboard
        } else {
            Self::Other(value)
        }
    }
}

/// A rectangular OpenRGB LED matrix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixMap {
    /// Number of matrix rows.
    pub height: u32,
    /// Number of matrix columns.
    pub width: u32,
    /// Controller LED indices in row-major order; holes are `None`.
    pub leds: Vec<Option<usize>>,
}

/// A controller zone and its optional matrix metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zone {
    /// Zone name reported by OpenRGB.
    pub name: String,
    /// Number of LEDs in the zone.
    pub led_count: u32,
    /// Matrix metadata, when provided by the controller.
    pub matrix: Option<MatrixMap>,
}

/// An OpenRGB controller discovered from the SDK server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Controller {
    /// Protocol device identifier used for controller commands.
    pub id: u32,
    /// Device category.
    pub device_type: DeviceType,
    /// Controller name.
    pub name: String,
    /// Controller vendor.
    pub vendor: String,
    /// Controller serial number, when available.
    pub serial: String,
    /// Controller zones.
    pub zones: Vec<Zone>,
    /// Current full-device LED colors.
    pub colors: Vec<Color>,
}

/// A synchronous connection to an OpenRGB SDK server.
pub struct Client {
    stream: TcpStream,
    protocol_version: u32,
}

impl Client {
    /// Connects to an OpenRGB SDK server and negotiates a protocol version.
    pub fn connect(
        address: impl ToSocketAddrs,
        client_name: &str,
        timeout: Duration,
    ) -> Result<Self> {
        let address = address
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| OpenRgbError::Protocol("server address resolved to nothing".into()))?;
        let stream = TcpStream::connect_timeout(&address, timeout)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        stream.set_nodelay(true)?;

        let mut client = Self {
            stream,
            protocol_version: 0,
        };
        client.negotiate_protocol()?;
        client.set_client_name(client_name)?;
        Ok(client)
    }

    /// Returns the negotiated protocol version.
    pub fn protocol_version(&self) -> u32 {
        self.protocol_version
    }

    /// Requests all controllers currently registered with OpenRGB.
    pub fn controllers(&mut self) -> Result<Vec<Controller>> {
        self.send_packet(0, REQUEST_CONTROLLER_COUNT, &[])?;
        let count_packet = self.read_expected(REQUEST_CONTROLLER_COUNT)?;
        let mut count_reader = PacketReader::new(&count_packet.payload);
        let count = count_reader.u32()?;
        let count = usize::try_from(count)
            .map_err(|_| OpenRgbError::Protocol("controller count is too large".into()))?;

        let mut controllers = Vec::with_capacity(count);
        for index in 0..count {
            let id = u32::try_from(index)
                .map_err(|_| OpenRgbError::Protocol("controller index is too large".into()))?;
            let version = self.protocol_version.to_le_bytes();
            self.send_packet(id, REQUEST_CONTROLLER_DATA, &version)?;
            let packet = self.read_expected(REQUEST_CONTROLLER_DATA)?;
            controllers.push(parse_controller(
                id,
                self.protocol_version,
                &packet.payload,
            )?);
        }
        Ok(controllers)
    }

    /// Switches a controller to its custom/direct per-LED mode.
    pub fn set_custom_mode(&mut self, controller_id: u32) -> Result<()> {
        self.send_packet(controller_id, SET_CUSTOM_MODE, &[])
    }

    /// Replaces all LED colors on one controller in a single SDK packet.
    pub fn update_leds(&mut self, controller_id: u32, colors: &[Color]) -> Result<()> {
        let color_count = u16::try_from(colors.len())
            .map_err(|_| OpenRgbError::Protocol("controller has more than 65535 LEDs".into()))?;
        let data_size = 4usize
            .checked_add(2)
            .and_then(|value| value.checked_add(colors.len().saturating_mul(4)))
            .ok_or_else(|| OpenRgbError::Protocol("LED update size overflow".into()))?;
        let data_size = u32::try_from(data_size)
            .map_err(|_| OpenRgbError::Protocol("LED update is too large".into()))?;

        let mut payload = Vec::with_capacity(usize::try_from(data_size).unwrap_or_default());
        payload.extend_from_slice(&data_size.to_le_bytes());
        payload.extend_from_slice(&color_count.to_le_bytes());
        for color in colors {
            payload.extend_from_slice(&color.packed().to_le_bytes());
        }
        self.send_packet(controller_id, UPDATE_LEDS, &payload)
    }

    fn negotiate_protocol(&mut self) -> Result<()> {
        self.send_packet(
            0,
            REQUEST_PROTOCOL_VERSION,
            &MAX_SUPPORTED_PROTOCOL.to_le_bytes(),
        )?;
        let packet = self.read_expected(REQUEST_PROTOCOL_VERSION)?;
        let mut reader = PacketReader::new(&packet.payload);
        let server_version = reader.u32()?;
        self.protocol_version = server_version.min(MAX_SUPPORTED_PROTOCOL);
        if self.protocol_version == 0 {
            return Err(OpenRgbError::Protocol(
                "protocol version 0 is not supported".into(),
            ));
        }
        Ok(())
    }

    fn set_client_name(&mut self, name: &str) -> Result<()> {
        if name.as_bytes().contains(&0) {
            return Err(OpenRgbError::Protocol(
                "client name contains a NUL byte".into(),
            ));
        }
        let mut payload = Vec::with_capacity(name.len() + 1);
        payload.extend_from_slice(name.as_bytes());
        payload.push(0);
        self.send_packet(0, SET_CLIENT_NAME, &payload)
    }

    fn send_packet(&mut self, device_id: u32, packet_id: u32, payload: &[u8]) -> Result<()> {
        if payload.len() > MAX_PACKET_SIZE {
            return Err(OpenRgbError::Protocol(format!(
                "packet payload exceeds {} bytes",
                MAX_PACKET_SIZE
            )));
        }
        let payload_len = u32::try_from(payload.len())
            .map_err(|_| OpenRgbError::Protocol("packet payload is too large".into()))?;
        let mut header = [0u8; HEADER_SIZE];
        header[0..4].copy_from_slice(&MAGIC);
        header[4..8].copy_from_slice(&device_id.to_le_bytes());
        header[8..12].copy_from_slice(&packet_id.to_le_bytes());
        header[12..16].copy_from_slice(&payload_len.to_le_bytes());
        self.stream.write_all(&header)?;
        self.stream.write_all(payload)?;
        Ok(())
    }

    fn read_expected(&mut self, expected_packet_id: u32) -> Result<Packet> {
        loop {
            let packet = self.read_packet()?;
            if packet.packet_id == expected_packet_id {
                return Ok(packet);
            }
            if packet.packet_id != 100 {
                return Err(OpenRgbError::Protocol(format!(
                    "expected packet {}, received {}",
                    expected_packet_id, packet.packet_id
                )));
            }
        }
    }

    fn read_packet(&mut self) -> Result<Packet> {
        let mut header = [0u8; HEADER_SIZE];
        self.stream.read_exact(&mut header)?;
        if header[0..4] != MAGIC {
            return Err(OpenRgbError::Protocol("invalid packet magic".into()));
        }

        let packet_id = u32::from_le_bytes(header[8..12].try_into().unwrap_or_default());
        let payload_len = u32::from_le_bytes(header[12..16].try_into().unwrap_or_default());
        let payload_len = usize::try_from(payload_len)
            .map_err(|_| OpenRgbError::Protocol("packet size is too large".into()))?;
        if payload_len > MAX_PACKET_SIZE {
            return Err(OpenRgbError::Protocol(format!(
                "packet payload exceeds {} bytes",
                MAX_PACKET_SIZE
            )));
        }
        let mut payload = vec![0; payload_len];
        self.stream.read_exact(&mut payload)?;
        Ok(Packet { packet_id, payload })
    }
}

struct Packet {
    packet_id: u32,
    payload: Vec<u8>,
}

fn parse_controller(id: u32, protocol_version: u32, payload: &[u8]) -> Result<Controller> {
    let mut outer = PacketReader::new(payload);
    let data_size = outer.u32()? as usize;
    let body_size = data_size.checked_sub(4).ok_or_else(|| {
        OpenRgbError::Protocol("controller data size is smaller than its header".into())
    })?;
    if body_size > outer.remaining() {
        return Err(OpenRgbError::Protocol(
            "controller data size exceeds packet payload".into(),
        ));
    }
    let data = outer.bytes(body_size)?;
    let mut reader = PacketReader::new(data);

    let device_type = DeviceType::from(reader.i32()?);
    let name = reader.string()?;
    let vendor = if protocol_version >= 1 {
        reader.string()?
    } else {
        String::new()
    };
    let _description = reader.string()?;
    let _version = reader.string()?;
    let serial = reader.string()?;
    let _location = reader.string()?;

    let mode_count = reader.u16()?;
    let _active_mode = reader.i32()?;
    for _ in 0..mode_count {
        skip_mode(&mut reader, protocol_version)?;
    }

    let zone_count = reader.u16()?;
    let mut zones = Vec::with_capacity(usize::from(zone_count));
    for _ in 0..zone_count {
        zones.push(parse_zone(&mut reader, protocol_version)?);
    }

    let led_count = reader.u16()?;
    for _ in 0..led_count {
        let _led_name = reader.string()?;
        let _led_value = reader.u32()?;
    }

    let color_count = reader.u16()?;
    let mut colors = Vec::with_capacity(usize::from(color_count));
    for _ in 0..color_count {
        colors.push(Color::from_packed(reader.u32()?));
    }

    if protocol_version >= 5 {
        let display_name_count = reader.u16()?;
        for _ in 0..display_name_count {
            let _display_name = reader.string()?;
        }
        let _flags = reader.u32()?;
    }

    Ok(Controller {
        id,
        device_type,
        name,
        vendor,
        serial,
        zones,
        colors,
    })
}

fn skip_mode(reader: &mut PacketReader<'_>, protocol_version: u32) -> Result<()> {
    let _name = reader.string()?;
    let _value = reader.i32()?;
    let _flags = reader.u32()?;
    let _speed_min = reader.u32()?;
    let _speed_max = reader.u32()?;
    if protocol_version >= 3 {
        let _brightness_min = reader.u32()?;
        let _brightness_max = reader.u32()?;
    }
    let _colors_min = reader.u32()?;
    let _colors_max = reader.u32()?;
    let _speed = reader.u32()?;
    if protocol_version >= 3 {
        let _brightness = reader.u32()?;
    }
    let _direction = reader.u32()?;
    let _color_mode = reader.u32()?;
    let color_count = reader.u16()?;
    for _ in 0..color_count {
        let _color = reader.u32()?;
    }
    Ok(())
}

fn parse_zone(reader: &mut PacketReader<'_>, protocol_version: u32) -> Result<Zone> {
    let name = reader.string()?;
    let _zone_type = reader.i32()?;
    let _leds_min = reader.u32()?;
    let _leds_max = reader.u32()?;
    let led_count = reader.u32()?;
    let matrix_len = usize::from(reader.u16()?);
    let matrix = if matrix_len == 0 {
        None
    } else {
        Some(parse_matrix(reader.bytes(matrix_len)?)?)
    };

    if protocol_version >= 4 {
        let segment_count = reader.u16()?;
        for _ in 0..segment_count {
            let _name = reader.string()?;
            let _segment_type = reader.i32()?;
            let _start_index = reader.u32()?;
            let _segment_led_count = reader.u32()?;
        }
    }
    if protocol_version >= 5 {
        let _flags = reader.u32()?;
    }

    Ok(Zone {
        name,
        led_count,
        matrix,
    })
}

fn parse_matrix(bytes: &[u8]) -> Result<MatrixMap> {
    let mut reader = PacketReader::new(bytes);
    let height = reader.u32()?;
    let width = reader.u32()?;
    let entry_count = height
        .checked_mul(width)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| OpenRgbError::Protocol("matrix dimensions overflow".into()))?;
    let expected_bytes = entry_count
        .checked_mul(4)
        .ok_or_else(|| OpenRgbError::Protocol("matrix size overflow".into()))?;
    if reader.remaining() != expected_bytes {
        return Err(OpenRgbError::Protocol(format!(
            "matrix contains {} bytes, expected {}",
            reader.remaining(),
            expected_bytes
        )));
    }

    let mut leds = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        let index = reader.u32()?;
        leds.push((index != u32::MAX).then_some(index as usize));
    }
    Ok(MatrixMap {
        height,
        width,
        leds,
    })
}

struct PacketReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> PacketReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.position)
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(len)
            .ok_or_else(|| OpenRgbError::Protocol("packet offset overflow".into()))?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| OpenRgbError::Protocol("packet ended unexpectedly".into()))?;
        self.position = end;
        Ok(value)
    }

    fn u16(&mut self) -> Result<u16> {
        let bytes: [u8; 2] = self.bytes(2)?.try_into().unwrap_or_default();
        Ok(u16::from_le_bytes(bytes))
    }

    fn u32(&mut self) -> Result<u32> {
        let bytes: [u8; 4] = self.bytes(4)?.try_into().unwrap_or_default();
        Ok(u32::from_le_bytes(bytes))
    }

    fn i32(&mut self) -> Result<i32> {
        let bytes: [u8; 4] = self.bytes(4)?.try_into().unwrap_or_default();
        Ok(i32::from_le_bytes(bytes))
    }

    fn string(&mut self) -> Result<String> {
        let len = usize::from(self.u16()?);
        if len == 0 {
            return Ok(String::new());
        }
        let bytes = self.bytes(len)?;
        let bytes = bytes
            .strip_suffix(&[0])
            .ok_or_else(|| OpenRgbError::Protocol("OpenRGB string is not NUL-terminated".into()))?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| OpenRgbError::Protocol("OpenRGB string is not UTF-8".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, thread};

    #[test]
    fn color_roundtrip_matches_openrgb_layout() {
        let color = Color::new(0x12, 0x34, 0x56);
        assert_eq!(color.packed(), 0x0056_3412);
        assert_eq!(Color::from_packed(color.packed()), color);
    }

    #[test]
    fn parses_matrix_holes() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&3u32.to_le_bytes());
        for index in [0, 1, u32::MAX, 2, 3, 4] {
            bytes.extend_from_slice(&index.to_le_bytes());
        }

        let matrix = parse_matrix(&bytes).unwrap();
        assert_eq!(matrix.height, 2);
        assert_eq!(matrix.width, 3);
        assert_eq!(
            matrix.leds,
            vec![Some(0), Some(1), None, Some(2), Some(3), Some(4)]
        );
    }

    #[test]
    fn rejects_inconsistent_matrix_size() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());

        let error = parse_matrix(&bytes).unwrap_err();
        assert!(error.to_string().contains("expected 16"));
    }

    #[test]
    fn rejects_unterminated_strings() {
        let bytes = [3, 0, b'a', b'b', b'c'];
        let error = PacketReader::new(&bytes).string().unwrap_err();
        assert!(error.to_string().contains("NUL-terminated"));
    }

    #[test]
    fn negotiates_discovers_and_updates_against_mock_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();

            let request = read_test_packet(&mut stream);
            assert_eq!(request.packet_id, REQUEST_PROTOCOL_VERSION);
            assert_eq!(request.payload, MAX_SUPPORTED_PROTOCOL.to_le_bytes());
            write_test_packet(
                &mut stream,
                0,
                REQUEST_PROTOCOL_VERSION,
                &MAX_SUPPORTED_PROTOCOL.to_le_bytes(),
            );

            let request = read_test_packet(&mut stream);
            assert_eq!(request.packet_id, SET_CLIENT_NAME);
            assert_eq!(request.payload, b"test-client\0");

            let request = read_test_packet(&mut stream);
            assert_eq!(request.packet_id, REQUEST_CONTROLLER_COUNT);
            write_test_packet(
                &mut stream,
                0,
                REQUEST_CONTROLLER_COUNT,
                &1u32.to_le_bytes(),
            );

            let request = read_test_packet(&mut stream);
            assert_eq!(request.packet_id, REQUEST_CONTROLLER_DATA);
            assert_eq!(request.payload, MAX_SUPPORTED_PROTOCOL.to_le_bytes());
            let controller = test_controller_packet();
            write_test_packet(&mut stream, 0, REQUEST_CONTROLLER_DATA, &controller);

            let request = read_test_packet(&mut stream);
            assert_eq!(request.packet_id, SET_CUSTOM_MODE);
            assert!(request.payload.is_empty());

            let request = read_test_packet(&mut stream);
            assert_eq!(request.packet_id, UPDATE_LEDS);
            let mut reader = PacketReader::new(&request.payload);
            assert_eq!(reader.u32().unwrap(), 14);
            assert_eq!(reader.u16().unwrap(), 2);
            assert_eq!(reader.u32().unwrap(), Color::new(1, 2, 3).packed());
            assert_eq!(reader.u32().unwrap(), Color::new(4, 5, 6).packed());
        });

        let mut client = Client::connect(address, "test-client", Duration::from_secs(1)).unwrap();
        assert_eq!(client.protocol_version(), MAX_SUPPORTED_PROTOCOL);
        let controllers = client.controllers().unwrap();
        assert_eq!(controllers.len(), 1);
        assert_eq!(controllers[0].device_type, DeviceType::Keyboard);
        assert_eq!(controllers[0].name, "Mock Keyboard");
        assert_eq!(controllers[0].serial, "SERIAL-1");
        assert_eq!(controllers[0].zones[0].matrix.as_ref().unwrap().width, 2);

        client.set_custom_mode(controllers[0].id).unwrap();
        client
            .update_leds(
                controllers[0].id,
                &[Color::new(1, 2, 3), Color::new(4, 5, 6)],
            )
            .unwrap();
        server.join().unwrap();
    }

    fn test_controller_packet() -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&5i32.to_le_bytes());
        push_test_string(&mut body, "Mock Keyboard");
        push_test_string(&mut body, "Mock Vendor");
        push_test_string(&mut body, "Mock description");
        push_test_string(&mut body, "1.0");
        push_test_string(&mut body, "SERIAL-1");
        push_test_string(&mut body, "usb:1");
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&(-1i32).to_le_bytes());
        body.extend_from_slice(&1u16.to_le_bytes());
        push_test_string(&mut body, "Keys");
        body.extend_from_slice(&2i32.to_le_bytes());
        body.extend_from_slice(&4u32.to_le_bytes());
        body.extend_from_slice(&4u32.to_le_bytes());
        body.extend_from_slice(&4u32.to_le_bytes());
        body.extend_from_slice(&24u16.to_le_bytes());
        body.extend_from_slice(&2u32.to_le_bytes());
        body.extend_from_slice(&2u32.to_le_bytes());
        for index in 0..4u32 {
            body.extend_from_slice(&index.to_le_bytes());
        }
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&4u16.to_le_bytes());
        for index in 0..4u32 {
            push_test_string(&mut body, &format!("Key {index}"));
            body.extend_from_slice(&index.to_le_bytes());
        }
        body.extend_from_slice(&4u16.to_le_bytes());
        for _ in 0..4 {
            body.extend_from_slice(&0u32.to_le_bytes());
        }
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());

        let mut packet = Vec::with_capacity(body.len() + 4);
        packet.extend_from_slice(&u32::try_from(body.len() + 4).unwrap().to_le_bytes());
        packet.extend_from_slice(&body);
        packet
    }

    fn push_test_string(output: &mut Vec<u8>, value: &str) {
        let len = u16::try_from(value.len() + 1).unwrap();
        output.extend_from_slice(&len.to_le_bytes());
        output.extend_from_slice(value.as_bytes());
        output.push(0);
    }

    fn read_test_packet(stream: &mut TcpStream) -> Packet {
        let mut header = [0u8; HEADER_SIZE];
        stream.read_exact(&mut header).unwrap();
        assert_eq!(header[0..4], MAGIC);
        let packet_id = u32::from_le_bytes(header[8..12].try_into().unwrap());
        let size = u32::from_le_bytes(header[12..16].try_into().unwrap()) as usize;
        let mut payload = vec![0; size];
        stream.read_exact(&mut payload).unwrap();
        Packet { packet_id, payload }
    }

    fn write_test_packet(stream: &mut TcpStream, device_id: u32, packet_id: u32, payload: &[u8]) {
        stream.write_all(&MAGIC).unwrap();
        stream.write_all(&device_id.to_le_bytes()).unwrap();
        stream.write_all(&packet_id.to_le_bytes()).unwrap();
        stream
            .write_all(&u32::try_from(payload.len()).unwrap().to_le_bytes())
            .unwrap();
        stream.write_all(payload).unwrap();
    }
}
