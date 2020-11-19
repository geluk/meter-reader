use enc28j60::Enc28j60;
use embedded_hal::{digital::v1::OutputPin, blocking::spi::{Transfer, Write}};
use smoltcp::Result;
use smoltcp::phy::{self, DeviceCapabilities, Device};
use smoltcp::time::Instant;

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
        where F: FnOnce(&mut [u8]) -> Result<R>
    {
        f(&mut self.buffer)
    }
}

struct Enc28j60TxToken<'a, D> {
    buffer: &'a mut [u8],
    driver: &'a mut D,
}

impl<'a, D: Driver> phy::TxToken for Enc28j60TxToken<'a, D> {
    fn consume<R, F>(mut self, _timestamp: Instant, len: usize, f: F) -> Result<R>
        where F: FnOnce(&mut [u8]) -> Result<R>
    {
        let result = f(&mut self.buffer[..len]);
        self.driver.transmit(&self.buffer[..len]);
        result
    }
}
