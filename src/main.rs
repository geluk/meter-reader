#![no_std]
#![no_main]

#[macro_use]
mod macros;
mod network;

extern crate panic_halt;

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

    let mut driver = network::create_enc28j60(&mut systick, spi4, ncs, rst);
    get_packets(&mut driver);

    halt!();
}

fn get_packets<D: network::Driver>(driver: &mut D) {
    let mut packet_count = driver.pending_packets();
    log::info!("{} packets ready to be decoded.", packet_count);
    let mut buffer = [0u8; 1518];
    while packet_count > 0 {
        let len = driver.receive(&mut buffer) as usize;
        let dst = &buffer[0..6];
        let src = &buffer[6..12];
        let ethertype = &buffer[12..14];
        log::info!(
            "> DST {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            dst[0],
            dst[1],
            dst[2],
            dst[3],
            dst[4],
            dst[5],
        );
        log::info!(
            "> SRC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            src[0],
            src[1],
            src[2],
            src[3],
            src[4],
            src[5],
        );
        log::info!("> EtherType: {:02x}{:02x}", ethertype[0], ethertype[1]);
        log::info!("> Payload: \n{:02x?}", &buffer[14..len]);
        packet_count -= 1;
    }
}
