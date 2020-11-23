#![allow(deprecated)] // Required because enc28j60 depends on v1.

use core::convert::TryInto;

use crate::{clock::Clock, network::driver::Driver, Enc28j60Phy, Random};
use smoltcp::{
    dhcp::{self, Dhcpv4Client},
    iface::{EthernetInterfaceBuilder, NeighborCache, Routes},
    socket::{
        RawPacketMetadata, RawSocketBuffer, SocketRef, SocketSet, TcpSocket, TcpSocketBuffer,
    },
    time::Instant,
    wire::IpEndpoint,
    wire::{EthernetAddress, IpAddress, IpCidr, Ipv4Address},
};
use teensy4_bsp::SysTick;

pub struct BackingStore {
    tcp_rx_buffer: [u8; 8192],
    tcp_tx_buffer: [u8; 2048],
    dhcp_rx_buffer: [u8; 1024],
    dhcp_tx_buffer: [u8; 1024],
    dhcp_tx_metadata: [RawPacketMetadata; 4],
    dhcp_rx_metadata: [RawPacketMetadata; 4],
    // address_buffer: [IpCidr; 1],
}

impl BackingStore {
    pub fn new() -> Self {
        BackingStore {
            tcp_rx_buffer: [0; 8192],
            tcp_tx_buffer: [0; 2048],
            dhcp_rx_buffer: [0; 1024],
            dhcp_tx_buffer: [0; 1024],
            dhcp_tx_metadata: [RawPacketMetadata::EMPTY; 4],
            dhcp_rx_metadata: [RawPacketMetadata::EMPTY; 4],
            // address_buffer: [IpCidr::new(Ipv4Address::UNSPECIFIED.into(), 0)],
        }
    }
}

pub fn init_network<D: Driver>(
    driver: D,
    clock: &mut Clock,
    systick: &mut SysTick,
    random: &mut Random,
    store: &mut BackingStore,
    addr: [u8; 6],
) -> ! {
    log::info!("Starting network setup");
    let device = Enc28j60Phy::new(driver);
    let mut addresses = [IpCidr::new(Ipv4Address::UNSPECIFIED.into(), 0)];

    let mut cache_backing_store = [None; 8];
    let neigh_cache = NeighborCache::new(&mut cache_backing_store[..]);
    let eth_addr = EthernetAddress(addr);

    let mut routes_storage = [None; 1];
    let routes = Routes::new(&mut routes_storage[..]);

    let mut interface = EthernetInterfaceBuilder::new(device)
        .ethernet_addr(eth_addr)
        .neighbor_cache(neigh_cache)
        .ip_addrs(&mut addresses[..])
        .routes(routes)
        .finalize();

    let tcp_rx_buffer = TcpSocketBuffer::new(&mut store.tcp_rx_buffer[..]);
    let tcp_tx_buffer = TcpSocketBuffer::new(&mut store.tcp_tx_buffer[..]);

    let dhcp_rx_buffer = RawSocketBuffer::new(
        &mut store.dhcp_tx_metadata[..],
        &mut store.dhcp_rx_buffer[..],
    );
    let dhcp_tx_buffer = RawSocketBuffer::new(
        &mut store.dhcp_rx_metadata[..],
        &mut store.dhcp_tx_buffer[..],
    );

    let socket = TcpSocket::new(tcp_rx_buffer, tcp_tx_buffer);
    let mut socket_buffer = [None, None, None, None, None, None, None, None];
    let mut sockets = SocketSet::new(&mut socket_buffer[..]);
    let tcp_handle = sockets.add(socket);

    let mut dhcp = Dhcpv4Client::new(
        &mut sockets,
        dhcp_rx_buffer,
        dhcp_tx_buffer,
        clock.instant(),
    );

    let mut tcp_active = false;
    let mut conn_tried = false;

    let mut sent = false;
    loop {
        match interface.poll(&mut sockets, clock.instant()) {
            Ok(processed) => {
                if processed {
                    log::trace!(
                        "[{}] Processed/emitted new packets during polling",
                        clock.millis()
                    );
                }
            }
            Err(e) => {
                log::warn!("Error during polling: {:?}", e);
            }
        }
        let dhcp_poll_res = dhcp.poll(&mut interface, &mut sockets, clock.instant());
        handle_dhcp(dhcp_poll_res, &mut interface);

        let socket = sockets.get::<TcpSocket>(tcp_handle);
        handle_tcpip(
            &mut interface,
            socket,
            random,
            &mut tcp_active,
            &mut conn_tried,
            &mut sent,
        );

        let now = clock.millis();
        let delay = interface
            .poll_at(&sockets, Instant::from_millis(now))
            .map_or(50, |t| t.total_millis() - now)
            .try_into()
            .unwrap_or(50);
        systick.delay(delay);
    }
}

fn generate_local_port(random: &mut Random) -> u16 {
    49152 + random.next(16383) as u16
}

fn handle_tcpip<D: for<'d> smoltcp::phy::Device<'d>>(
    interface: &mut smoltcp::iface::EthernetInterface<D>,
    mut socket: SocketRef<TcpSocket>,
    random: &mut Random,
    tcp_active: &mut bool,
    conn_tried: &mut bool,
    sent: &mut bool,
) {
    if socket.is_active() && !*tcp_active {
        let local = socket.local_endpoint();
        let remote = socket.remote_endpoint();
        log::debug!("Connected {} -> {}", local, remote);
    } else if !socket.is_active() && *tcp_active {
        let local = socket.local_endpoint();
        let remote = socket.remote_endpoint();
        log::debug!("Disconnected {} -> {}", local, remote);
    }
    *tcp_active = socket.is_active();

    let addr = interface.ipv4_addr().filter(|addr| !addr.is_unspecified());
    match addr {
        Some(addr) if !socket.is_active() && !*sent && !*conn_tried => {
            let local = IpEndpoint::new(addr.into(), generate_local_port(random));
            let remote = IpEndpoint::new(IpAddress::v4(10, 190, 10, 10), 8000);

            log::debug!("Got address, trying to connect {} -> {}", local, remote);
            let result = socket.connect(remote, local);
            *conn_tried = true;
            match result {
                Ok(_) => (),
                Err(err) => log::warn!("Failed to connect: {}", err),
            }
        }
        _ => {}
    }
    if socket.can_send() && !*sent {
        log::trace!("Sending data to host");
        let data = b"GET / HTTP/1.1\r\nHost: www.msftconnecttest.com\r\nUser-Agent: power-meter/smoltcp/0.1\r\nConnection: close\r\n\r\n";
        socket.send_slice(&data[..]).unwrap();
        *sent = true;
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

fn handle_dhcp<D: for<'d> smoltcp::phy::Device<'d>>(
    dhcp: smoltcp::Result<Option<dhcp::Dhcpv4Config>>,
    interface: &mut smoltcp::iface::EthernetInterface<D>,
) {
    match dhcp {
        Ok(Some(cfg)) => {
            log::info!(
                "Received DHCP configuration: {:?} via {:?}, DNS {:?}",
                cfg.address,
                cfg.router,
                cfg.dns_servers
            );
            match cfg.address {
                Some(cidr) => interface.update_ip_addrs(|addrs| {
                    let addr = addrs.iter_mut().next().unwrap();
                    log::info!("Received CIDR: {}", cidr);
                    *addr = IpCidr::Ipv4(cidr);
                }),
                None => log::warn!("Did not receive CIDR from DHCP"),
            }
            match cfg.router {
                Some(addr) => {
                    if let Some(route) =
                        interface.routes_mut().add_default_ipv4_route(addr).unwrap()
                    {
                        log::info!("Replaced previous route {} with {}", route.via_router, addr);
                    } else {
                        log::info!("Added new default route via {}", addr);
                    }
                }
                None => log::warn!("Did not receive router address from DHCP"),
            }
        }
        Err(err) => {
            log::warn!("DHCP error: {}", err);
        }
        _ => {}
    }
}
