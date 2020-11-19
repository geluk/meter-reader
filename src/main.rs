#![no_std]
#![no_main]

extern crate panic_halt;

use core::fmt::Write;

// use embedded_hal::blocking::spi::Write;
use cortex_m::interrupt::Mutex;
use embedded_hal::blocking::delay::DelayMs;
#[allow(deprecated)] // Required because enc28j60 depends on v1.
use embedded_hal::digital::v1::OutputPin;
use embedded_hal::digital::v2::OutputPin as OutputPinV2;
use embedded_hal::digital::v1_compat::OldOutputPin;

use teensy4_bsp as bsp;
use bsp::hal;
use bsp::hal::iomuxc::prelude::*;

// use enc28j60::Enc28j60;

//const SPI_BAUD_RATE_HZ: u32 = 1_000_000;
const SPI_BAUD_RATE_HZ: u32 = 500_000;

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

    log::info!("USB logging initialised.");

    // Configure the SPI clocks. We'll only use SPI4 for now.
    let (_, _, _, spi4_builder) = per.spi.clock(
        &mut per.ccm.handle,
        hal::ccm::spi::ClockSelect::Pll2,
        hal::ccm::spi::PrescalarSelect::LPSPI_PODF_5,
    );

    let pins = bsp::t40::into_pins(per.iomuxc);

    // Set SPI pin assignments.
    let mut spi4 = spi4_builder.build(
        pins.p11,
        pins.p12,
        pins.p13,
    );

    systick.delay(5000);

    // Set SPI clock speed.
    match spi4.set_clock_speed(hal::spi::ClockSpeed(SPI_BAUD_RATE_HZ)) {
        Ok(()) => {
            log::info!("Set SPI clock speed to {} Hz.", SPI_BAUD_RATE_HZ);
        }
        Err(err) => {
            log::warn!("Unable to set SPI clock speed: {:?}", err);
        }
    }

    // Enable the peripheral-controlled chip select 0.
    // This lets the chip select be controlled by the hardware,
    // and means that we won't need to pass a chip select pin
    // to the enc28j60, so we'll pass it a dummy instead.
    spi4.enable_chip_select_0(pins.p10);

    // Create a GPIO output pin.
    let mut gpio = bsp::hal::gpio::GPIO::new(pins.p9).output();
    gpio.set_fast(true);

    // spi_test(&mut systick, spi4, op);
    //net_setup(&mut systick, spi4, op);

    halt!();
}

// // SPI testing code
// #[allow(unused)]
// fn spi_test<Mod>(
//     delay: &mut impl embedded_hal::blocking::delay::DelayMs<u16>,
//     mut spi: hal::spi::SPI<impl consts::Unsigned>,
//     mut op: impl OutputPinV2)
// {
//     // Dump some test data.
//     loop {
//         for i in 0..255_u8 {
//             match spi.write(&[i]) {
//                 Ok(()) => {
//                     log::info!("Wrote byte: {}", i);
//                 }
//                 Err(err) => {
//                     log::warn!("Write failed: {:?}", err);
//                 }
//             };
//         }
//         op.set_low();
//         delay.delay_ms(250);
//         op.set_high();
//         delay.delay_ms(250);
//     }
// }

// // ENC28J60 support (WIP)
// #[allow(deprecated, unused)] // 'deprecated' required because enc28j60 depends on v1.
// fn net_setup(
//     delay: &mut bsp::SysTick,
//     spi: hal::spi::SPI<impl consts::Unsigned>,
//     mut rst: impl OutputPinV2<Error = impl core::fmt::Debug> + 'static)
// {
//     struct DummyCS;
//     impl OutputPin for DummyCS {
//         fn set_high(&mut self) {}
//         fn set_low(&mut self) {}
//     }

//     log::info!("Initialising reset pin.");
//     if let Err(err) = rst.set_high() {
//         log::warn!("Failed to write to reset pin: {:?}", err);
//         halt!();
//     }
//     delay.delay_ms(10_u16);

//     let oop = OldOutputPin::new(rst);
    
//     log::info!("Setting up ENC28J60.");
//     let enc28j60 = Enc28j60::new(
//         spi,
//         DummyCS,
//         enc28j60::Unconnected, // Interrupt
//         oop,
//         delay,
//         7 * KB,
//         MAC.0,
//     );
//     log::info!("Setup done.");
//     match enc28j60 {
//         Ok(mut enc) => {
//             log::info!("ENC ready!");
//             //return;
//             loop {
//                 get_packets(&mut enc);
//                 delay.delay(2000);
//             }
//         }
//         Err(err) => {
//             log::warn!("Failed to initialise ENC: {:?}", err);
//         }
//     };
// }

// #[allow(deprecated)]
// fn get_packets<NCS, INT, RESET>(
//     enc: &mut Enc28j60<
//         hal::spi::SPI<impl consts::Unsigned>,
//         NCS,
//         INT,
//         RESET>,
// ) where
//     NCS: OutputPin,
//     INT: enc28j60::IntPin,
//     RESET: enc28j60::ResetPin,
// {
//     match enc.pending_packets() {
//         Ok(pkt) => {
//             log::info!("{} packets ready to be decoded.", pkt);
//         }
//         Err(err) => {
//             log::warn!("Failed to get packet count from ENC: {:?}", err);
//         }
//     }
// }
