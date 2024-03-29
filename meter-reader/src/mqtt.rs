use arrayvec::ArrayString;
use core::fmt::{Debug, Display};
use dsmr42::Telegram;
use embedded_mqtt::{
    codec::{Decodable, Encodable},
    fixed_header::PacketType,
    fixed_header::PublishFlags,
    packet::Packet,
    payload,
    status::Status,
    variable_header::connect::Flags,
    variable_header::VariableHeader,
    variable_header::{
        self, connack,
        connect::{Level, Protocol},
    },
};
use smoltcp::{
    iface::EthernetInterface,
    phy,
    socket::{SocketHandle, SocketRef, TcpSocket},
    time::Duration,
    wire::IpAddress,
    wire::IpEndpoint,
    wire::Ipv4Address,
};

use crate::{network::client::TcpClient, network::stack, random::Random};

const REMOTE_HOST: [u8; 4] = [10, 190, 30, 14];
const REMOTE_PORT: u16 = 1883;

const BACKOFF_CAP: u32 = 400000;
const INITIAL_BACKOFF: u32 = 1000;

const KEEPALIVE: u16 = 30;

const CLIENT_ID: &str = "smart-meter-reader";

const STATUS_TOPIC: &str = "smart_meter/status";
const USAGE_TOPIC: &str = "smart_meter/usage";

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum MqttState {
    Unconnected,
    Connecting,
    Connected,
    Ready,
    Invalid,
}

impl Display for MqttState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        Debug::fmt(self, f)
    }
}

pub struct MqttClient {
    handle: Option<SocketHandle>,
    connected: bool,
    next_backoff: u32,
    current_backoff: u32,
    mqtt_state: MqttState,
    queued_telegram: Option<Telegram>,
}

impl TcpClient for MqttClient {
    fn set_socket_handle(&mut self, handle: SocketHandle) {
        self.handle = Some(handle);
    }
    fn get_socket_handle(&mut self) -> SocketHandle {
        self.handle.unwrap()
    }
    fn poll<DeviceT>(
        &mut self,
        _interface: &mut EthernetInterface<DeviceT>,
        mut socket: SocketRef<TcpSocket>,
        random: &mut Random,
    ) where
        DeviceT: for<'d> phy::Device<'d>,
    {
        // A connection is considered established if we can send data.
        // However, it is only considered closed once we are no longer exchanging packets.
        // Because of this we track both states here.
        if socket.may_send() && !self.connected {
            self.connected = true;
            self.next_backoff = INITIAL_BACKOFF;
            self.current_backoff = 0;
            log::debug!(
                "Connected {} -> {}, keepalive {:?}, timeout {:?}",
                socket.local_endpoint(),
                socket.remote_endpoint(),
                socket.keep_alive(),
                socket.timeout(),
            );
        } else if !socket.is_active() && self.connected {
            self.connected = false;
            self.mqtt_state = MqttState::Unconnected;
            log::debug!(
                "Disconnected {} -> {}",
                socket.local_endpoint(),
                socket.remote_endpoint()
            );
        }

        if !socket.is_active() {
            self.try_connect(socket, random);
            return;
        }

        if socket.can_recv() {
            let recv_res = socket.recv(|buf| match Packet::decode(buf) {
                Ok(Status::Complete((len, pkt))) => (len, Some(pkt)),
                Ok(Status::Partial(_)) => {
                    log::info!("Got partial MQTT packet, retrying later.");
                    (0, None)
                }
                Err(err) => {
                    log::warn!("Decode error: {}", err);
                    (buf.len(), None)
                }
            });
            match recv_res {
                Ok(Some(pkt)) => self.handle_packet(pkt),
                Err(err) => log::warn!("Failed to receive MQTT packet: {}", err),
                _ => {}
            }
        }

        if socket.can_send() {
            match self.mqtt_state {
                MqttState::Unconnected => self.connect_mqtt(socket),
                MqttState::Connected => self.send_status(socket),
                MqttState::Ready => {
                    if let Some(telegram) = self.queued_telegram.take() {
                        self.send_telegram(socket, telegram);
                    }
                }
                _ => {}
            }
        }
    }
}

impl MqttClient {
    pub fn new() -> Self {
        Self {
            handle: None,
            connected: false,
            next_backoff: INITIAL_BACKOFF,
            current_backoff: 0,
            mqtt_state: MqttState::Unconnected,
            queued_telegram: None,
        }
    }

    fn connect_mqtt(&mut self, socket: SocketRef<TcpSocket>) {
        log::debug!("Creating MQTT connect request");
        self.mqtt_state = MqttState::Connecting;
        let mut flags = Flags::default();
        flags.set_clean_session(true);
        flags.set_has_will_flag(true);
        flags.set_will_retain(true);
        let header = variable_header::connect::Connect::new(
            Protocol::MQTT,
            Level::Level3_1_1,
            flags,
            KEEPALIVE,
        );
        let will = payload::connect::Will::new(STATUS_TOPIC, b"offline");
        let payload = payload::connect::Connect::new(CLIENT_ID, Some(will), None, None);
        match Packet::connect(header, payload) {
            Ok(packet) => match self.send_packet(socket, packet) {
                Ok(_) => log::debug!("Sent MQTT connect request"),
                Err(err) => log::warn!("Failed to send connect packet: {}", err),
            },
            Err(err) => log::warn!("Failed to create connect packet: {}", err),
        }
    }

    pub fn send_status(&mut self, socket: SocketRef<TcpSocket>) {
        self.send_pub(socket, STATUS_TOPIC, b"online");
        log::debug!("MQTT State: Connected -> Ready");
        self.mqtt_state = MqttState::Ready;
    }

    pub fn queue_telegram(&mut self, telegram: Telegram) {
        self.queued_telegram = Some(telegram);
    }

    fn send_telegram(&mut self, socket: SocketRef<TcpSocket>, telegram: Telegram) {
        let mut content = ArrayString::<512>::new();

        telegram.serialize(&mut content);

        self.send_pub(socket, USAGE_TOPIC, content.as_bytes());
    }

    fn send_pub(&mut self, socket: SocketRef<TcpSocket>, topic: &str, payload: &[u8]) {
        log::info!("Publishing {} bytes to {}", payload.len(), topic);
        let header = variable_header::publish::Publish::new(topic, None);

        let mut flags = PublishFlags::default();
        flags.set_retain(true);
        match Packet::publish(flags, header, payload).map(|p| self.send_packet(socket, p)) {
            Err(err) => log::warn!("Failed to encode publish packet: {}", err),
            Ok(Err(err)) => log::warn!("Failed to send publish packet: {}", err),
            Ok(Ok(())) => {}
        }
    }

    fn send_packet(
        &mut self,
        mut socket: SocketRef<TcpSocket>,
        packet: Packet,
    ) -> smoltcp::Result<()> {
        log::info!("Sending {:?}: {:?}", packet.fixed_header().r#type(), packet);
        socket.send(|buf| match packet.encode(buf) {
            Ok(bytes) => {
                log::info!("Sent {} bytes", bytes);
                (bytes, ())
            }
            Err(err) => {
                log::warn!("Failed to decode connect packet: {}", err);
                (0, ())
            }
        })
    }

    fn handle_packet(&mut self, packet: Packet) {
        log::debug!("{:#?}", packet);
        match packet.fixed_header().r#type() {
            PacketType::Connack => self.handle_connack(packet),
            PacketType::Pingresp => {}
            _ => self.invalid_packet(packet),
        }
    }

    fn invalid_packet(&mut self, packet: Packet) {
        log::warn!(
            "Received invalid packet for state {}:\n{:#?}",
            self.mqtt_state,
            packet
        );
        self.mqtt_state = MqttState::Invalid;
    }

    fn handle_connack(&mut self, packet: Packet) {
        if self.mqtt_state != MqttState::Connecting {
            log::warn!(
                "Received unexpected CONNACK, current state: {}",
                self.mqtt_state
            );
            self.mqtt_state = MqttState::Invalid;
            return;
        }
        match packet.variable_header() {
            Some(VariableHeader::Connack(connack)) => match connack.return_code() {
                connack::ReturnCode::Accepted => {
                    log::debug!("MQTT State: Connecting -> Connected");
                    self.mqtt_state = MqttState::Connected;
                }
                other => {
                    log::warn!("MQTT Connection request denied: {:?}", other);
                    self.mqtt_state = MqttState::Invalid;
                }
            },
            _ => self.invalid_packet(packet),
        }
    }

    fn try_connect(&mut self, mut socket: SocketRef<TcpSocket>, random: &mut Random) {
        if self.current_backoff > 0 {
            self.current_backoff -= 1;
            return;
        }
        socket.set_timeout(Some(Duration::from_secs(120)));
        socket.set_keep_alive(Some(Duration::from_secs(30)));
        self.current_backoff = self.next_backoff;
        self.next_backoff = self.next_backoff.saturating_mul(2).min(BACKOFF_CAP);

        let local = stack::generate_local_port(random);
        let remote = IpAddress::Ipv4(Ipv4Address(REMOTE_HOST));
        let remote = IpEndpoint::new(remote, REMOTE_PORT);
        log::debug!(
            "Socket inactive, trying to connect 0.0.0.0:{} -> {}, backoff {} if connect fails",
            local,
            remote,
            self.current_backoff,
        );
        let result = socket.connect(remote, local);
        if let Err(err) = result {
            log::warn!("Failed to connect: {}", err);
        }
    }
}
