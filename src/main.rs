#![no_std]
#![no_main]

extern crate panic_halt;

use embedded_hal::blocking::spi::Write;
use embedded_hal::blocking::delay::DelayMs;
#[allow(deprecated)] // Required because enc28j60 depends on v1.
use embedded_hal::digital::v1::OutputPin;
use embedded_hal::digital::v2::OutputPin as OutputPinV2;
use embedded_hal::digital::v1_compat::OldOutputPin;

use teensy4_bsp as bsp;
use bsp::hal;
use bsp::rt;

use hal::iomuxc::spi::module::Module as SpiModule;

use enc28j60::Enc28j60;

use jnet::{ipv4, mac};

const MAC: mac::Addr = mac::Addr([0x22, 0x22, 0x00, 0x00, 0x00, 0x00]);
const IP: ipv4::Addr = ipv4::Addr([192, 168, 1, 101]);

//const SPI_BAUD_RATE_HZ: u32 = 1_000_000;
const SPI_BAUD_RATE_HZ: u32 = 500_000;

const KB: u16 = 1024;

macro_rules! halt {
    () => {
        loop {}
    };
}

#[rt::entry]
fn main() -> ! {
    // Take control of the peripherals.
    let mut per = bsp::Peripherals::take().unwrap();

    let bsp::Peripherals {
        usb,
        spi,
        mut systick,
        mut gpr,
        ..
    } = per;

    // Enable serial USB logging.
    usb.init(Default::default());

    // Set the default clock speed (500MHz).
    per.ccm
        .pll1
        .set_arm_clock(hal::ccm::PLL1::ARM_HZ, &mut per.ccm.handle, &mut per.dcdc);

    // Configure the SPI clocks. We'll only use SPI4 for now.
    let (_, _, _, spi4_builder) = spi.clock(
        &mut per.ccm.handle,
        hal::ccm::spi::ClockSelect::Pll2,
        hal::ccm::spi::PrescalarSelect::LPSPI_PODF_5,
    );

    // Set SPI pin assignments.
    let mut spi4 = spi4_builder.build(
        per.pins.p11.alt3(),
        per.pins.p12.alt3(),
        per.pins.p13.alt3(),
    );

    systick.delay(5000);

    // Set SPI clock speed.
    match spi4.set_clock_speed(hal::spi::ClockSpeed(SPI_BAUD_RATE_HZ)) {
        Ok(()) => {
            log::info!("Set clock speed to {}Hz.", SPI_BAUD_RATE_HZ);
        }
        Err(err) => {
            log::warn!("Unable to set clock speed: {:?}", err);
        }
    }

    // Enable the peripheral-controlled chip select 0.
    // This lets the chip select be controlled by the hardware,
    // and means that we won't need to pass a chip select pin
    // to the enc280j60, so we'll pass it a dummy instead.
    spi4.enable_chip_select_0(per.pins.p10.alt3());

    // Create a GPIO output pin.
    use hal::gpio::IntoGpio;
    let op = per.pins.p9.alt5().into_gpio().fast(&mut gpr).output();

    //spi_test(&mut systick, spi4, op);
    net_setup(&mut systick, spi4, op);

    halt!();
}

// SPI testing code
#[allow(unused)]
fn spi_test<Mod>(
    delay: &mut impl embedded_hal::blocking::delay::DelayMs<u16>,
    mut spi: hal::spi::SPI<impl SpiModule>,
    mut op: impl OutputPinV2)
{
    // Dump some test data.
    loop {
        for i in 0..255_u8 {
            match spi.write(&[i]) {
                Ok(()) => {
                    log::info!("Wrote byte: {}", i);
                }
                Err(err) => {
                    log::warn!("Write failed: {:?}", err);
                }
            };
        }
        op.set_low();
        delay.delay_ms(250);
        op.set_high();
        delay.delay_ms(250);
    }
}

// ENC28J60 support (WIP)
#[allow(deprecated, unused)] // 'deprecated' required because enc28j60 depends on v1.
fn net_setup(
    delay: &mut bsp::SysTick,
    spi: hal::spi::SPI<impl SpiModule>,
    mut rst: impl OutputPinV2<Error = impl core::fmt::Debug>)
{
    struct DummyCS;
    impl OutputPin for DummyCS {
        fn set_high(&mut self) {}
        fn set_low(&mut self) {}
    }

    log::info!("Initialising reset pin.");
    if let Err(err) = rst.set_high() {
        log::warn!("Failed to write to reset pin: {:?}", err);
        halt!();
    }
    delay.delay_ms(10_u16);

    let oop = OldOutputPin::new(rst);
    
    log::info!("Setting up ENC28J60.");
    let enc28j60 = Enc28j60::new(
        spi,
        DummyCS,
        enc28j60::Unconnected, // Interrupt
        oop,
        delay,
        7 * KB,
        MAC.0,
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
fn get_packets<NCS, INT, RESET>(
    enc: &mut Enc28j60<
        hal::spi::SPI<impl SpiModule>,
        NCS,
        INT,
        RESET>,
) where
    NCS: OutputPin,
    INT: enc28j60::IntPin,
    RESET: enc28j60::ResetPin,
{
    match enc.pending_packets() {
        Ok(pkt) => {
            log::info!("{} packets ready to be decoded.", pkt);
        }
        Err(err) => {
            log::warn!("Failed to get packet count from ENC: {:?}", err);
        }
    }
}
