#![no_std]
#![no_main]

mod clock;
#[macro_use]
mod macros;
mod network;

extern crate panic_halt;

use crate::network::driver::create_enc28j60;
use crate::network::driver::Enc28j60Phy;
use crate::network::main::init_network;
use bsp::{
    hal::{self, gpio::GPIO},
    t40, usb, SysTick,
};
use clock::Clock;
use embedded_hal::digital::v1_compat::OldOutputPin;
use hal::ccm::{
    spi::{ClockSelect, PrescalarSelect},
    PLL1,
};

use teensy4_bsp as bsp;
use teensy4_bsp::usb::LoggingConfig;

const SPI_BAUD_RATE_HZ: u32 = 16_000_000;
const ETH_ADDR: [u8; 6] = [0x22, 0x22, 0x00, 0x00, 0x00, 0x00];

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

    let driver = create_enc28j60(&mut systick, spi4, ncs, rst, ETH_ADDR);
    let mut store = network::main::BackingStore::new();
    init_network(driver, &mut clock, &mut systick, &mut store, ETH_ADDR);
}
