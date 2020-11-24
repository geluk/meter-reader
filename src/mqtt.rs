use smoltcp::{iface::EthernetInterface, wire::IpAddress, phy, socket::{SocketHandle, SocketRef, TcpSocket}, wire::IpEndpoint};

use crate::{network::client::TcpClient, random::Random, network::stack};

pub struct MqttClient {
    handle: Option<SocketHandle>,
    tcp_active: bool,
    data_sent: bool,
    conn_tried: bool,
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
        if socket.is_active() && !self.tcp_active {
            let local = socket.local_endpoint();
            let remote = socket.remote_endpoint();
            log::debug!("Connected {} -> {}", local, remote);
        } else if !socket.is_active() && self.tcp_active {
            let local = socket.local_endpoint();
            let remote = socket.remote_endpoint();
            log::debug!("Disconnected {} -> {}", local, remote);
        }
        self.tcp_active = socket.is_active();

        if !socket.is_active() && !self.data_sent && !self.conn_tried {
            let local = stack::generate_local_port(random);
            let remote = IpEndpoint::new(IpAddress::v4(10, 190, 10, 10), 8000);

            log::debug!(
                "Got address, trying to connect 0.0.0.0:{} -> {}",
                local,
                remote
            );
            let result = socket.connect(remote, local);
            self.conn_tried = true;
            match result {
                Ok(_) => (),
                Err(err) => log::warn!("Failed to connect: {}", err),
            }
        }

        // Not exactly MQTT, but that will come later...
        if socket.can_send() && !self.data_sent {
            log::trace!("Sending data to host");
            let data = b"GET / HTTP/1.1\r\nHost: www.msftconnecttest.com\r\nUser-Agent: power-meter/smoltcp/0.1\r\nConnection: close\r\n\r\n";
            socket.send_slice(&data[..]).unwrap();
            self.data_sent = true;
        }
        if socket.can_recv() {
            log::info!("Socket has data");
            socket
                .recv(|data| {
                    if !data.is_empty() {
                        let msg = core::str::from_utf8(data).unwrap_or("(invalid utf8)");
                        log::info!("Received reply:\n{}", msg);
                        (data.len(), ())
                    } else {
                        log::info!("Received empty");
                        (0, ())
                    }
                })
                .unwrap();
        }

        if socket.may_send() && !socket.may_recv() {
            log::trace!("Remote endpoint closed, closing socket.");
            // Remote endpoint closed their half of the connection, we should close ours too.
            socket.close();
        }
    }
}

impl MqttClient {
    pub fn new() -> Self {
        Self {
             handle: None,
            conn_tried: false,
            data_sent: false,
            tcp_active: false,
            }
    }
    
    pub fn do_work(&mut self) {
    }
}
