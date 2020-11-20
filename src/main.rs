#![no_std]
#![no_main]

#[macro_use]
mod macros;
mod network;

extern crate panic_halt;

use smoltcp::time::Instant;
use smoltcp::socket::SocketSet;
use smoltcp::socket::TcpSocketBuffer;
use smoltcp::socket::TcpSocket;
use smoltcp::wire::IpCidr;
use smoltcp::wire::EthernetAddress;
use smoltcp::iface::EthernetInterfaceBuilder;
use smoltcp::iface::NeighborCache;
use core::str::FromStr;
use smoltcp::wire::IpAddress;
use bsp::{
    hal::{self, gpio::GPIO},
    t40, usb, SysTick,
};
use embedded_hal::digital::v1_compat::OldOutputPin;
use hal::ccm::{
    spi::{ClockSelect, PrescalarSelect},
    PLL1,
};
use teensy4_bsp as bsp;

const ETH_ADDR: [u8; 6] = [0x22, 0x22, 0x00, 0x00, 0x00, 0x00];
const SPI_BAUD_RATE_HZ: u32 = 16_000_000;

#[cortex_m_rt::entry]
fn main() -> ! {
    // Take control of the peripherals.
    let mut per = bsp::Peripherals::take().unwrap();
    let core_per = cortex_m::Peripherals::take().unwrap();

    // Enable serial USB logging.
    let mut systick = SysTick::new(core_per.SYST);
    let _ = usb::init(&systick, Default::default()).unwrap();

    // Set the default clock speed (500MHz).
    per.ccm
        .pll1
        .set_arm_clock(PLL1::ARM_HZ, &mut per.ccm.handle, &mut per.dcdc);

    systick.delay(5000);
    log::info!("USB logging initialised.");

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
            log::info!("Set SPI clock speed to {} Hz.", SPI_BAUD_RATE_HZ);
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
    let mut address = [IpCidr::new(IpAddress::v4(10, 111, 0, 1), 24)];
    let port = 9494;

    let mut cache_backing_store = [None; 8];
    let neigh = NeighborCache::new(&mut cache_backing_store[..]);
    let eth_addr = EthernetAddress(ETH_ADDR);

    let mut iface = EthernetInterfaceBuilder::new(device)
        .ethernet_addr(eth_addr)
        .neighbor_cache(neigh)
        .ip_addrs(&mut address[..])
        .finalize();

    let mut tcp_rx_buffer = [0u8; 64];
    let mut tcp_tx_buffer = [0u8; 64];
    let tcp_rx_buffer = TcpSocketBuffer::new(&mut tcp_rx_buffer[..]);
    let tcp_tx_buffer = TcpSocketBuffer::new(&mut tcp_tx_buffer[..]);

    let socket = TcpSocket::new(tcp_rx_buffer, tcp_tx_buffer);
    let mut socket_buffer = [None; 1];
    let mut sockets = SocketSet::new(&mut socket_buffer[..]);
    let tcp_handle = sockets.add(socket);

    let mut tcp_active = false;

    {
        let mut socket = sockets.get::<TcpSocket>(tcp_handle);
        socket
            .connect((IpAddress::v4(10, 111, 0, 2), port), 49500)
            .unwrap();
    }

    let mut millis = 0;
    let mut sent = false;
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

        if socket.can_send() && !sent {
            log::debug!("Sending data");
            let data = b"abcdefghijklmnop";
            socket.send_slice(&data[..]).unwrap();
            socket.close();
            sent = true;
        }

        systick.delay(50);
        millis += 50;
        if millis % 1000 == 0 {
            log::debug!("Still running...");
        }
    }
}
