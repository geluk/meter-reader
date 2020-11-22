#![allow(deprecated)] // Required because enc28j60 depends on v1.

use crate::network::driver::Driver;
use core::convert::TryInto;

use crate::clock::Clock;
use crate::Enc28j60Phy;

use teensy4_bsp::SysTick;

use smoltcp::dhcp::Dhcpv4Client;
use smoltcp::iface::Routes;
use smoltcp::{
    iface::{EthernetInterfaceBuilder, NeighborCache},
    socket::{RawPacketMetadata, RawSocketBuffer, SocketSet, TcpSocket, TcpSocketBuffer},
    time::Instant,
    wire::{EthernetAddress, IpAddress, IpCidr, Ipv4Address},
};

pub fn init_network<D>(
    driver: D,
    clock: &mut Clock,
    systick: &mut SysTick,
    addr: [u8; 6],
) -> !
where
    D: Driver + 'static
{
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

    let mut tcp_rx_buffer = [0u8; 64];
    let mut tcp_tx_buffer = [0u8; 64];
    let tcp_rx_buffer = TcpSocketBuffer::new(&mut tcp_rx_buffer[..]);
    let tcp_tx_buffer = TcpSocketBuffer::new(&mut tcp_tx_buffer[..]);

    let mut dhcp_rx_buffer = [0u8; 900];
    let mut dhcp_tx_buffer = [0u8; 600];
    let mut tx_metadata = [RawPacketMetadata::EMPTY; 4];
    let mut rx_metadata = [RawPacketMetadata::EMPTY; 4];
    let dhcp_rx_buffer = RawSocketBuffer::new(&mut tx_metadata[..], &mut dhcp_rx_buffer[..]);
    let dhcp_tx_buffer = RawSocketBuffer::new(&mut rx_metadata[..], &mut dhcp_tx_buffer[..]);

    let socket = TcpSocket::new(tcp_rx_buffer, tcp_tx_buffer);
    let mut socket_buffer = [None, None];
    let mut sockets = SocketSet::new(&mut socket_buffer[..]);
    let tcp_handle = sockets.add(socket);

    let mut dhcp = Dhcpv4Client::new(
        &mut sockets,
        dhcp_rx_buffer,
        dhcp_tx_buffer,
        Instant::from_millis(0),
    );

    let mut tcp_active = false;
    let mut conn_tried = false;

    let mut millis = 0;
    let mut sent = false;
    log::info!("Starting network setup");
    loop {
        let timestamp = Instant::from_millis(millis);
        match interface.poll(&mut sockets, timestamp) {
            Ok(processed) => {
                if processed {
                    log::trace!("Processed/emitted new packets during polling");
                }
            }
            Err(e) => {
                log::warn!("Error during polling: {:?}", e);
            }
        }
        match dhcp.poll(&mut interface, &mut sockets, timestamp) {
            Ok(dhcp) => {
                if let Some(cfg) = dhcp {
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
                                log::info!(
                                    "Replaced previous route {} with {}",
                                    route.via_router,
                                    addr
                                );
                            } else {
                                log::info!("Added new default route via {}", addr);
                            }
                        }
                        None => log::warn!("Did not receive router address from DHCP"),
                    }
                }
            }
            Err(err) => {
                log::warn!("DHCP error: {}", err);
            }
        }
        {
            let mut socket = sockets.get::<TcpSocket>(tcp_handle);
            if socket.is_active() && !tcp_active {
                let local = socket.local_endpoint();
                let remote = socket.remote_endpoint();
                log::debug!("Connected {} -> {}", local, remote);
            } else if !socket.is_active() && tcp_active {
                let local = socket.local_endpoint();
                let remote = socket.remote_endpoint();
                log::debug!("Disconnected {} -> {}", local, remote);
            }
            tcp_active = socket.is_active();

            let addr = interface.ipv4_addr().filter(|addr| !addr.is_unspecified());
            match addr {
                Some(addr) if !socket.is_active() && !sent && !conn_tried => {
                    log::debug!("Got address: {}, trying to connect", addr);
                    let result =
                        socket.connect((IpAddress::v4(10, 190, 10, 11), 9494), (addr, 45000));
                    conn_tried = true;
                    match result {
                        Ok(_) => (),
                        Err(err) => log::warn!("Failed to connect: {}", err),
                    }
                }
                _ => {}
            }
            if socket.can_send() && !sent {
                log::debug!("Sending data to host");
                let data = b"abcdefghijklmnop";
                socket.send_slice(&data[..]).unwrap();
                socket.close();
                sent = true;
            }
        }
        millis += 1;
        let timestamp = Instant::from_millis(millis);
        let next_poll = interface
            .poll_at(&sockets, timestamp)
            .map_or(50, |t| t.total_millis())
            .try_into()
            .unwrap_or(u32::MAX);

        systick.delay(next_poll);
        millis += next_poll;
        if millis % 1000 == 0 {
            log::debug!("Still running...");
            log::debug!("Clock at {}", clock.millis());
        }
    }
}
