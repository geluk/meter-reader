#![allow(deprecated)] // Required because enc28j60 depends on v1.

use smoltcp::{
    dhcp::{Dhcpv4Client, Dhcpv4Config},
    iface::{EthernetInterface, EthernetInterfaceBuilder, Neighbor, NeighborCache, Route, Routes},
    socket::{
        RawPacketMetadata, RawSocketBuffer, SocketSet, SocketSetItem, TcpSocket, TcpSocketBuffer,
    },
    wire::{EthernetAddress, IpAddress, IpCidr, Ipv4Address},
};

use crate::{clock::Clock, network::driver::Driver, Enc28j60Phy, Random};

use super::client::{TcpClient, TcpClientStore};

const EPHEMERAL_PORT_START: u16 = 49152;
const EPHEMERAL_PORT_COUNT: u16 = 16383;

const DHCP_RX_BUF_SZ: usize = 1024;
const DHCP_TX_BUF_SZ: usize = 1024;
const DHCP_RX_MET_SZ: usize = 4;
const DHCP_TX_MET_SZ: usize = 4;

const NEIGH_CACHE_SZ: usize = 64;

const SOCKET_STORE_SZ: usize = 2;

pub struct BackingStore<'store> {
    dhcp_rx_buffer: [u8; DHCP_RX_BUF_SZ],
    dhcp_tx_buffer: [u8; DHCP_TX_BUF_SZ],
    dhcp_rx_metadata: [RawPacketMetadata; DHCP_RX_MET_SZ],
    dhcp_tx_metadata: [RawPacketMetadata; DHCP_TX_MET_SZ],
    neigh_cache: [Option<(IpAddress, Neighbor)>; NEIGH_CACHE_SZ],
    address_store: [IpCidr; 1],
    route_store: [Option<(IpCidr, Route)>; 1],
    socket_store: [Option<SocketSetItem<'store>>; SOCKET_STORE_SZ],
}

impl<'store> BackingStore<'store> {
    pub fn new() -> Self {
        BackingStore {
            dhcp_rx_buffer: [0; DHCP_RX_BUF_SZ],
            dhcp_tx_buffer: [0; DHCP_TX_BUF_SZ],
            dhcp_rx_metadata: [RawPacketMetadata::EMPTY; DHCP_RX_MET_SZ],
            dhcp_tx_metadata: [RawPacketMetadata::EMPTY; DHCP_TX_MET_SZ],
            neigh_cache: [None; NEIGH_CACHE_SZ],
            address_store: [IpCidr::new(Ipv4Address::UNSPECIFIED.into(), 0)],
            route_store: [None; 1],
            socket_store: Default::default(),
        }
    }
}

pub struct NetworkStack<'store, D: Driver> {
    interface: EthernetInterface<'store, Enc28j60Phy<D>>,
    dhcp_client: Dhcpv4Client,
    sockets: SocketSet<'store>,
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

        let dhcp_rx_buffer = RawSocketBuffer::new(
            &mut store.dhcp_tx_metadata[..],
            &mut store.dhcp_rx_buffer[..],
        );
        let dhcp_tx_buffer = RawSocketBuffer::new(
            &mut store.dhcp_rx_metadata[..],
            &mut store.dhcp_tx_buffer[..],
        );
        let mut sockets = SocketSet::new(&mut store.socket_store[..]);

        let dhcp_client = Dhcpv4Client::new(
            &mut sockets,
            dhcp_rx_buffer,
            dhcp_tx_buffer,
            clock.instant(),
        );

        Self {
            interface,
            dhcp_client,
            sockets,
        }
    }

    pub fn add_client<C: TcpClient>(&mut self, client: &mut C, store: &'store mut TcpClientStore) {
        let socket = TcpSocket::new(
            TcpSocketBuffer::new(&mut store.rx_buffer[..]),
            TcpSocketBuffer::new(&mut store.tx_buffer[..]),
        );
        client.set_socket_handle(self.sockets.add(socket));
    }

    pub fn poll(&mut self, clock: &mut Clock) -> Option<i64> {
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
            Err(err) if err == smoltcp::Error::Malformed => {
                // This will happen from time to time on most networks,
                // so we shouldn't let it pollute our logs.
                log::trace!("Malformed DHCP packet");
            }
            Err(err) if err == smoltcp::Error::Unrecognized => {
                // Same as with Malformed.
                log::trace!("Unrecognised DHCP packet");
            }
            Err(err) => log::warn!("DHCP error: {}", err),
            _ => {}
        }

        self.interface
            .poll_at(&self.sockets, clock.instant())
            .map(|t| t.total_millis())
    }

    pub fn poll_client<C: TcpClient>(&mut self, random: &mut Random, client: &mut C) {
        // Only handle TCP/IP if we have a valid address
        let addr = self.interface.ipv4_addr();
        if addr.is_some() && !addr.unwrap().is_unspecified() {
            let socket = client.get_socket_handle();
            let socket = self.sockets.get(socket);
            client.poll(&mut self.interface, socket, random);
        }
    }

    fn handle_dhcp(&mut self, cfg: Dhcpv4Config) {
        log::info!(
            "Received DHCP configuration: {:?} via {:?}, DNS {:?}",
            cfg.address,
            cfg.router,
            cfg.dns_servers
        );

        match cfg {
            Dhcpv4Config {
                address: Some(cidr),
                router: Some(router),
                ..
            } => {
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
                    log::info!(
                        "Replaced previous route {} with {}",
                        prev_route.via_router,
                        router
                    );
                } else {
                    log::info!("Added new default route via {}", router);
                }
            }
            cfg => {
                log::warn!(
                    "DHCP configuration did not contain address or DNS: {:?}",
                    cfg
                );
            }
        }
    }
}

#[inline]
pub fn generate_local_port(random: &mut Random) -> u16 {
    EPHEMERAL_PORT_START + random.next(EPHEMERAL_PORT_COUNT as u32) as u16
}
