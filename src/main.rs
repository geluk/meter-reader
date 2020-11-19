#![no_std]
#![no_main]

mod network;

extern crate panic_halt;

#[allow(deprecated)] // Required because enc28j60 depends on v1.
use embedded_hal::digital::v1_compat::OldOutputPin;
use embedded_hal::digital::v2::OutputPin as OutputPinV2;

use bsp::hal;
use bsp::hal::spi;
use teensy4_bsp as bsp;

use enc28j60::Enc28j60;

//const SPI_BAUD_RATE_HZ: u32 = 1_000_000;
const SPI_BAUD_RATE_HZ: u32 = 2_000_000;

const KB: u16 = 1024;

macro_rules! halt {
    () => {
        loop {}
    };
}

#[cortex_m_rt::entry]
fn main() -> ! {
    // Take control of the peripherals.
    let mut per = bsp::Peripherals::take().unwrap();
    let core_per = cortex_m::Peripherals::take().unwrap();

    // Enable serial USB logging.
    let mut systick = bsp::SysTick::new(core_per.SYST);
    let _ = bsp::usb::init(&systick, Default::default()).unwrap();

    // Set the default clock speed (500MHz).
    per.ccm
        .pll1
        .set_arm_clock(hal::ccm::PLL1::ARM_HZ, &mut per.ccm.handle, &mut per.dcdc);

    systick.delay(5000);
    log::info!("USB logging initialised.");

    // Configure the SPI clocks. We'll only use SPI4 for now.
    let (_, _, _, spi4_builder) = per.spi.clock(
        &mut per.ccm.handle,
        hal::ccm::spi::ClockSelect::Pll2,
        hal::ccm::spi::PrescalarSelect::LPSPI_PODF_5,
    );

    let pins = bsp::t40::into_pins(per.iomuxc);

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
    let mut rst = bsp::hal::gpio::GPIO::new(pins.p9).output();
    rst.set_fast(true);
    let mut ncs = bsp::hal::gpio::GPIO::new(pins.p10).output();
    ncs.set_fast(true);

    //spi_test(&mut systick, spi4, gpio);
    net_setup(&mut systick, spi4, ncs, rst);

    halt!();
}

// SPI testing code
#[allow(unused)]
fn spi_test<SPI, P>(
    delay: &mut impl embedded_hal::blocking::delay::DelayMs<u16>,
    mut spi: SPI,
    mut op: hal::gpio::GPIO<P, hal::gpio::Output>,
) where
    SPI: embedded_hal::blocking::spi::Transfer<u8, Error = spi::Error>,
    P: bsp::hal::iomuxc::gpio::Pin,
{
    // Dump some test data.
    loop {
        for i in 0..255_u8 {
            match spi.transfer(&mut [i]) {
                Ok(_) => {
                    log::info!("Wrote byte: {}", i);
                }
                Err(err) => {
                    log::warn!("Write failed: {:?}", err);
                }
            };
        }
        op.clear();
        delay.delay_ms(250);
        op.set();
        delay.delay_ms(250);
    }
}

// ENC28J60 support (WIP)
#[allow(deprecated, unused)] // 'deprecated' required because enc28j60 depends on v1.
fn net_setup<SPI, PNCS, PRST>(
    delay: &mut bsp::SysTick,
    mut spi: SPI,
    mut ncs: hal::gpio::GPIO<PNCS, hal::gpio::Output>,
    mut rst: hal::gpio::GPIO<PRST, hal::gpio::Output>,
) where
    SPI: embedded_hal::blocking::spi::write::Default<u8>
        + embedded_hal::blocking::spi::transfer::Default<u8>,
    SPI::Error: core::fmt::Debug,
    PNCS: bsp::hal::iomuxc::gpio::Pin + 'static,
    PRST: bsp::hal::iomuxc::gpio::Pin + 'static,
{
    log::info!("Initialising reset pin.");
    if let Err(err) = rst.set_high() {
        log::warn!("Failed to write to reset pin: {:?}", err);
        halt!();
    }
    delay.delay(1);
    let rst = OldOutputPin::new(rst);
    let ncs = OldOutputPin::new(ncs);

    log::info!("Setting up ENC28J60.");
    let enc28j60 = Enc28j60::new(
        spi,
        ncs,
        enc28j60::Unconnected, // Interrupt
        rst,
        delay,
        7 * KB,
        [0x22, 0x22, 0, 0, 0, 0],
    );
    log::info!("Setup done.");
    match enc28j60 {
        Ok(mut enc) => {
            log::info!("ENC ready!");
            //return;
            loop {
                get_packets(&mut enc);
                delay.delay(2000);
            }
        }
        Err(err) => {
            log::warn!("Failed to initialise ENC: {:?}", err);
        }
    };
}

#[allow(deprecated)]
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
