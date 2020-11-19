use enc28j60::Enc28j60;

use embedded_hal::{digital::v1::OutputPin, blocking::spi::{Transfer, Write}};

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
