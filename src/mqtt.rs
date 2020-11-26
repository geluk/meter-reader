use smoltcp::{
    iface::EthernetInterface,
    phy,
    socket::{SocketHandle, SocketRef, TcpSocket},
    wire::IpAddress,
    wire::IpEndpoint,
};

use crate::{network::client::TcpClient, network::stack, random::Random};

const BACKOFF_CAP: u32 = 400000;
const INITIAL_BACKOFF: u32 = 1000;

pub struct MqttClient {
    handle: Option<SocketHandle>,
    connected: bool,
    next_backoff: u32,
    current_backoff: u32,
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
        interface: &mut EthernetInterface<DeviceT>,
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
            let local = socket.local_endpoint();
            let remote = socket.remote_endpoint();
            log::debug!("Connected {} -> {}", local, remote);
        } else if !socket.is_active() && self.connected {
            self.connected = false;
            let local = socket.local_endpoint();
            let remote = socket.remote_endpoint();
            log::debug!("Disconnected {} -> {}", local, remote);
        }

        if !socket.is_active() {
            self.try_connect(socket, random);
            return;
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
        }
    }

    pub fn do_work(&mut self) {

    }

    fn try_connect(&mut self, mut socket: SocketRef<TcpSocket>, random: &mut Random) {
        if self.current_backoff > 0 {
            self.current_backoff -= 1;
            return;
        }
        self.current_backoff = self.next_backoff;
        self.next_backoff = self.next_backoff.saturating_mul(2).min(BACKOFF_CAP);

        let local = stack::generate_local_port(random);
        let remote = IpEndpoint::new(IpAddress::v4(10, 190, 10, 10), 8000);
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
