#![allow(deprecated)] // Required because enc28j60 depends on v1.

use core::fmt::Debug;

use embedded_hal::{
    blocking::spi,
    blocking::spi::{Transfer, Write},
    digital::v1::OutputPin,
};
use enc28j60::Enc28j60;
use smoltcp::phy::{self, Device, DeviceCapabilities};
use smoltcp::time::Instant;
use smoltcp::Result;
use teensy4_bsp::{hal::gpio, hal::iomuxc::gpio::Pin, SysTick};

const KB: u16 = 1024;

pub trait Driver {
    fn pending_packets(&mut self) -> u8;

    fn receive(&mut self, buffer: &mut [u8]) -> u16;

    fn transmit(&mut self, bytes: &[u8]);
}

impl<SPI, NCS, INT, RESET, E> Driver for Enc28j60<SPI, NCS, INT, RESET>
where
    SPI: Transfer<u8, Error = E> + Write<u8, Error = E>,
    NCS: OutputPin,
    INT: enc28j60::IntPin,
    RESET: enc28j60::ResetPin,
    E: core::fmt::Debug,
{
    #[inline]
    fn pending_packets(&mut self) -> u8 {
        Enc28j60::pending_packets(self).unwrap()
    }

    #[inline]
    fn receive(&mut self, buffer: &mut [u8]) -> u16 {
        Enc28j60::receive(self, buffer).unwrap()
    }

    #[inline]
    fn transmit(&mut self, bytes: &[u8]) {
        Enc28j60::transmit(self, bytes).unwrap()
    }
}

pub fn create_enc28j60<SPI, PNCS, PRST>(
    delay: &mut SysTick,
    spi: SPI,
    ncs: PNCS,
    mut rst: PRST,
) -> Enc28j60<SPI, PNCS, enc28j60::Unconnected, PRST>
where
    SPI: spi::write::Default<u8> + spi::transfer::Default<u8>,
    SPI::Error: Debug,
    PNCS: OutputPin + 'static,
    PRST: OutputPin + 'static,
{
    // Ensure the reset pin is high on startup
    rst.set_high();
    delay.delay(1);

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
        Ok(enc) => enc,
        Err(err) => {
            log::warn!("Failed to initialise ENC: {:?}", err);
            halt!();
        }
    }
}

pub struct Enc28j60Phy<D: Driver> {
    rx_buffer: [u8; 1518],
    tx_buffer: [u8; 1518],
    driver: D,
}

impl<D: Driver> Enc28j60Phy<D> {
    fn new(driver: D) -> Self {
        Self {
            rx_buffer: [0; 1518],
            tx_buffer: [0; 1518],
            driver,
        }
    }
}

struct Enc28j60RxToken<'a> {
    buffer: &'a mut [u8],
}

impl<'a> phy::RxToken for Enc28j60RxToken<'a> {
    fn consume<R, F>(mut self, _timestamp: Instant, f: F) -> Result<R>
    where
        F: FnOnce(&mut [u8]) -> Result<R>,
    {
        f(&mut self.buffer)
    }
}

struct Enc28j60TxToken<'a, D> {
    buffer: &'a mut [u8],
    driver: &'a mut D,
}

impl<'a, D: Driver> phy::TxToken for Enc28j60TxToken<'a, D> {
    fn consume<R, F>(self, _timestamp: Instant, len: usize, f: F) -> Result<R>
    where
        F: FnOnce(&mut [u8]) -> Result<R>,
    {
        if len > self.buffer.len() {
            return Err(smoltcp::Error::Exhausted);
        }
        let result = f(&mut self.buffer[..len]);
        self.driver.transmit(&self.buffer[..len]);
        result
    }
}
