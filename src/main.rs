#![no_std]
#![no_main]

mod clock;
#[macro_use]
mod macros;
mod network;

extern crate panic_halt;

use bsp::{
    hal::{self, gpio::GPIO},
    t40, usb, SysTick,
};
use clock::Clock;
use core::convert::TryInto;
use embedded_hal::digital::v1_compat::OldOutputPin;
use hal::ccm::{
    spi::{ClockSelect, PrescalarSelect},
    PLL1,
};
use smoltcp::dhcp::Dhcpv4Client;
use smoltcp::iface::Routes;
use smoltcp::{
    iface::{EthernetInterfaceBuilder, NeighborCache},
    socket::{RawPacketMetadata, RawSocketBuffer, SocketSet, TcpSocket, TcpSocketBuffer},
    time::Instant,
    wire::{EthernetAddress, IpAddress, IpCidr, Ipv4Address},
};
use teensy4_bsp as bsp;
use teensy4_bsp::usb::LoggingConfig;

const ETH_ADDR: [u8; 6] = [0x22, 0x22, 0x00, 0x00, 0x00, 0x00];
const SPI_BAUD_RATE_HZ: u32 = 16_000_000;

#[cortex_m_rt::entry]
fn main() -> ! {
    // Take control of the peripherals.
    let mut per = bsp::Peripherals::take().unwrap();
    let core_per = cortex_m::Peripherals::take().unwrap();

    // Enable serial USB logging.
    let mut systick = SysTick::new(core_per.SYST);
    let _ = usb::init(
        &systick,
        LoggingConfig {
            max_level: log::LevelFilter::Debug,
            filters: &[],
        },
    )
    .unwrap();

    systick.delay(5000);
    log::info!("USB logging initialised");

    // Set the default clock speed (500MHz).
    let (_, ipg) = per
        .ccm
        .pll1
        .set_arm_clock(PLL1::ARM_HZ, &mut per.ccm.handle, &mut per.dcdc);
    let mut clock = Clock::init(per.ccm.perclk, ipg, &mut per.ccm.handle, per.gpt2);
    log::info!("Current ms: {}", clock.millis());
    systick.delay(1000);
    log::info!("Current ms: {}", clock.millis());

    // Configure the SPI clocks. We'll only use SPI4 for now.
    let (_, _, _, spi4_builder) = per.spi.clock(
        &mut per.ccm.handle,
        ClockSelect::Pll2,
        PrescalarSelect::LPSPI_PODF_5,
    );

    let pins = t40::into_pins(per.iomuxc);

    // Set SPI pin assignments.
    let mut spi4 = spi4_builder.build(pins.p11, pins.p12, pins.p13);

    // Set SPI clock speed.
    match spi4.set_clock_speed(hal::spi::ClockSpeed(SPI_BAUD_RATE_HZ)) {
        Ok(()) => {
            log::info!("Set SPI clock speed to {} Hz", SPI_BAUD_RATE_HZ);
        }
        Err(err) => {
            log::warn!("Unable to set SPI clock speed: {:?}", err);
        }
    }

    // Create a GPIO output pin.
    let mut rst = GPIO::new(pins.p9).output();
    rst.set_fast(true);
    let rst = OldOutputPin::new(rst);
    let mut ncs = GPIO::new(pins.p10).output();
    ncs.set_fast(true);
    let ncs = OldOutputPin::new(ncs);

    let driver = network::create_enc28j60(&mut systick, spi4, ncs, rst, ETH_ADDR);
    let device = network::Enc28j60Phy::new(driver);
    let mut addresses = [IpCidr::new(Ipv4Address::UNSPECIFIED.into(), 0)];

    let mut cache_backing_store = [None; 8];
    let neigh_cache = NeighborCache::new(&mut cache_backing_store[..]);
    let eth_addr = EthernetAddress(ETH_ADDR);

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
