#![no_std]
#![no_main]

extern crate panic_halt;

use embedded_hal::digital::v1::OutputPin;
use embedded_hal::blocking::spi::Write;

use teensy4_bsp as bsp;
use bsp::rt;
use bsp::hal;
use jnet::{mac, ipv4};

const MAC: mac::Addr = mac::Addr([0x22, 0x22, 0x00, 0x00, 0x00, 0x00]);
const IP: ipv4::Addr = ipv4::Addr([192, 168, 1, 101]);

const SPI_BAUD_RATE_HZ: u32 = 1_000_000;

const KB: u16 = 1024;

#[rt::entry]
fn main() -> ! {
    // Take control of the peripherals.
    let mut per = bsp::Peripherals::take().unwrap();

    // Enable serial USB logging.
    per.usb.init(Default::default());

    // Set the default clock speed (500MHz).
    per.ccm.pll1.set_arm_clock(
        hal::ccm::PLL1::ARM_HZ,
        &mut per.ccm.handle,
        &mut per.dcdc);

    // Configure SPI. We'll only use SPI4 for now.
    let(_,_,_,spi4_builder) = per.spi.clock(
        &mut per.ccm.handle,
        hal::ccm::spi::ClockSelect::Pll2,
        hal::ccm::spi::PrescalarSelect::LPSPI_PODF_5);
    
    // Set SPI pin assignments.
    let mut spi4 = spi4_builder.build(
        per.pins.p11.alt3(),
        per.pins.p12.alt3(),
        per.pins.p13.alt3());

    per.systick.delay(5000);

    // Set SPI clock speed.
    match spi4.set_clock_speed(hal::spi::ClockSpeed(SPI_BAUD_RATE_HZ)) {
        Ok(()) => {
            log::info!(
                "Set clock speed to {}Hz.",
                SPI_BAUD_RATE_HZ);
        },
        Err(err) => {
            log::warn!("Unable to set clock speed: {:?}", err);
        },
    }

    // Enable the peripheral-controlled chip select 0.
    // This lets the chip select be controlled by the hardware,
    // and means that we won't need to pass a chip select pin
    // to the enc280j60, so we'll pass it a dummy instead.
    spi4.enable_chip_select_0(per.pins.p10.alt3());

    // Dump some test data.
    loop {
        for i in 0..255_u8 {
            match spi4.write(&[i]) {
                Ok(()) => {
                    log::info!("Wrote byte: {}", i);
                },
                Err(err) => {
                    log::warn!("Failed to write byte: {:?}", err);
                }
            };
        }

        per.systick.delay(500);
    }
}

// ENC28J60 support (WIP)
fn net_setup<Mod>(mut per: bsp::Peripherals, spi: hal::spi::SPI<Mod>)
    where Mod: hal::iomuxc::spi::module::Module
{
    struct DummyCS;
    impl OutputPin for DummyCS {
        fn set_high(&mut self) {
        }
        fn set_low(&mut self) {
        }
    }
    let ncs = DummyCS;
    
    let enc28j60 = enc28j60::Enc28j60::new(
        spi,
        ncs,
        enc28j60::Unconnected, // Int
        enc28j60::Unconnected, // Reset
        &mut per.systick,
        7 * KB,
        MAC.0,
    );
    match enc28j60 {
        Ok(enc) => {
            log::info!("ENC ready!");
        },
        Err(err) => {
            log::info!("Failed to initialise ENC: {:?}", err);
        },
    };
}