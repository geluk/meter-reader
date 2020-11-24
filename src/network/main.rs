#![allow(deprecated)] // Required because enc28j60 depends on v1.

use smoltcp::dhcp::Dhcpv4Config;
use smoltcp::iface::Neighbor;
use smoltcp::iface::Route;
use smoltcp::socket::SocketHandle;
use smoltcp::socket::SocketSetItem;

use crate::{clock::Clock, network::driver::Driver, Enc28j60Phy, Random};
use smoltcp::{
    dhcp::{Dhcpv4Client},
    iface::EthernetInterface,
    iface::{EthernetInterfaceBuilder, NeighborCache, Routes},
    socket::{RawPacketMetadata, RawSocketBuffer, SocketSet, TcpSocket, TcpSocketBuffer},
    wire::IpEndpoint,
    wire::{EthernetAddress, IpAddress, IpCidr, Ipv4Address},
};

const EPHEMERAL_PORT_START: u16 = 49152;
const EPHEMERAL_PORT_COUNT: u16 = 16383;

pub struct BackingStore<'store> {
    tcp_rx_buffer: [u8; 8192],
    tcp_tx_buffer: [u8; 2048],
    dhcp_rx_buffer: [u8; 1024],
    dhcp_tx_buffer: [u8; 1024],
    dhcp_tx_metadata: [RawPacketMetadata; 4],
    dhcp_rx_metadata: [RawPacketMetadata; 4],
    neigh_cache: [Option<(IpAddress, Neighbor)>; 64],
    address_store: [IpCidr; 1],
    route_store: [Option<(IpCidr, Route)>; 1],
    socket_store: [Option<SocketSetItem<'store, 'store>>; 2],
}

impl<'store> BackingStore<'store> {
    pub fn new() -> Self {
        BackingStore {
            tcp_rx_buffer: [0; 8192],
            tcp_tx_buffer: [0; 2048],
            dhcp_rx_buffer: [0; 1024],
            dhcp_tx_buffer: [0; 1024],
            dhcp_tx_metadata: [RawPacketMetadata::EMPTY; 4],
            dhcp_rx_metadata: [RawPacketMetadata::EMPTY; 4],
            neigh_cache: [None; 64],
            address_store: [IpCidr::new(Ipv4Address::UNSPECIFIED.into(), 0)],
            route_store: [None; 1],
            socket_store: [None, None],
        }
    }
}

pub struct NetworkStack<'store, D: Driver> {
    interface: EthernetInterface<'store, 'store, 'store, Enc28j60Phy<D>>,
    dhcp_client: Dhcpv4Client,
    tcp_handle: SocketHandle,
    sockets: SocketSet<'store, 'store, 'store>,
    tcp_active: bool,
    conn_tried: bool,
    data_sent: bool,
}

impl<'store, D: Driver> NetworkStack<'store, D> {
    pub fn new(
        driver: D,
        clock: &mut Clock,
        store: &'store mut BackingStore<'store>,
        addr: [u8; 6],
    ) -> NetworkStack<'store, D> {
        log::info!("Starting network setup");
        let device = Enc28j60Phy::new(driver);
        let eth_addr = EthernetAddress(addr);
        let neigh_cache = NeighborCache::new(&mut store.neigh_cache[..]);
        let routes = Routes::new(&mut store.route_store[..]);

        let interface = EthernetInterfaceBuilder::new(device)
            .ethernet_addr(eth_addr)
            .neighbor_cache(neigh_cache)
            .ip_addrs(&mut store.address_store[..])
            .routes(routes)
            .finalize();

        let tcp_rx_buffer = TcpSocketBuffer::new(&mut store.tcp_rx_buffer[..]);
        let tcp_tx_buffer = TcpSocketBuffer::new(&mut store.tcp_tx_buffer[..]);
        let socket = TcpSocket::new(tcp_rx_buffer, tcp_tx_buffer);

        let dhcp_rx_buffer = RawSocketBuffer::new(
            &mut store.dhcp_tx_metadata[..],
            &mut store.dhcp_rx_buffer[..],
        );
        let dhcp_tx_buffer = RawSocketBuffer::new(
            &mut store.dhcp_rx_metadata[..],
            &mut store.dhcp_tx_buffer[..],
        );
        let mut sockets = SocketSet::new(&mut store.socket_store[..]);
        let tcp_handle = sockets.add(socket);

        let dhcp_client = Dhcpv4Client::new(
            &mut sockets,
            dhcp_rx_buffer,
            dhcp_tx_buffer,
            clock.instant(),
        );

        Self {
            interface,
            dhcp_client,
            tcp_handle,
            sockets,
            tcp_active: false,
            conn_tried: false,
            data_sent: false,
        }
    }
    pub fn poll(&mut self, clock: &mut Clock, random: &mut Random) -> Option<i64> {
        self._poll(clock, random)
    }

    fn _poll(&mut self, clock: &mut Clock, random: &mut Random) -> Option<i64> {
        match self.interface.poll(&mut self.sockets, clock.instant()) {
            Ok(processed) if processed => {
                log::trace!("Processed/emitted new packets during polling");
            }
            Err(e) => {
                log::warn!("Error during polling: {:?}", e);
            }
            _ => {}
        }
        match self
            .dhcp_client
            .poll(&mut self.interface, &mut self.sockets, clock.instant())
        {
            Ok(Some(config)) => self.handle_dhcp(config),
            Err(err) => log::warn!("DHCP error: {}", err),
            _ => {}
        }

        self.handle_tcpip(random);

        self.interface
            .poll_at(&self.sockets, clock.instant())
            .map(|t| t.total_millis())
    }

    fn handle_dhcp(&mut self, cfg: Dhcpv4Config) {
        log::info!(
            "Received DHCP configuration: {:?} via {:?}, DNS {:?}",
            cfg.address,
            cfg.router,
            cfg.dns_servers
        );

        match cfg {
            Dhcpv4Config{ address: Some(cidr), router: Some(router), .. } => {
                self.interface.update_ip_addrs(|addrs| {
                    let addr = addrs.iter_mut().next().unwrap();
                    log::info!("Received CIDR: {}", cidr);
                    *addr = IpCidr::Ipv4(cidr);
                });
                if let Some(prev_route) = self
                    .interface
                    .routes_mut()
                    .add_default_ipv4_route(router)
                    .unwrap()
                {
                    log::info!("Replaced previous route {} with {}", prev_route.via_router, router);
                } else {
                    log::info!("Added new default route via {}", router);
                }
            },
            cfg => {
                log::warn!("DHCP configuration did not contain address or DNS: {:?}", cfg);
            },
        }
    }

    fn handle_tcpip(&mut self, random: &mut Random) {
        let mut socket = self.sockets.get::<TcpSocket>(self.tcp_handle);
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

        let addr = self
            .interface
            .ipv4_addr()
            .filter(|addr| !addr.is_unspecified());
        match addr {
            Some(addr) if !socket.is_active() && !self.data_sent && !self.conn_tried => {
                let local = IpEndpoint::new(addr.into(), Self::generate_local_port(random));
                let remote = IpEndpoint::new(IpAddress::v4(10, 190, 10, 10), 8000);

                log::debug!("Got address, trying to connect {} -> {}", local, remote);
                let result = socket.connect(remote, local);
                self.conn_tried = true;
                match result {
                    Ok(_) => (),
                    Err(err) => log::warn!("Failed to connect: {}", err),
                }
            }
            _ => {}
        }
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

    #[inline]
    fn generate_local_port(random: &mut Random) -> u16 {
        EPHEMERAL_PORT_START + random.next(EPHEMERAL_PORT_COUNT as u32) as u16
    }
}
