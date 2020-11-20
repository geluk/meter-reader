#![allow(deprecated)] // Required because enc28j60 depends on v1.

use core::fmt::Debug;

use embedded_hal::{
    blocking::spi,
    blocking::spi::{Transfer, Write},
    digital::v1::OutputPin,
};
use enc28j60::Enc28j60;
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::phy::{self, Device, DeviceCapabilities};
use smoltcp::time::Instant;
use smoltcp::Result;
use teensy4_bsp::SysTick;

const ENC28J60_MTU: usize = enc28j60::MAX_FRAME_LENGTH as usize;

pub trait Driver {
    fn pending_packets(&mut self) -> u8;

    fn receive(&mut self, buffer: &mut [u8]) -> u16;

    fn transmit(&mut self, buffer: &[u8]);
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
        let recv = Enc28j60::receive(self, buffer).unwrap();
        log::trace!("> Received {} bytes: {:2x?}", recv, &buffer[0..recv as usize]);
        recv
    }

    #[inline]
    fn transmit(&mut self, buffer: &[u8]) {
        Enc28j60::transmit(self, buffer).unwrap();
        log::trace!("> Sent {} bytes: {:2x?}", buffer.len(), buffer);
    }
}

pub fn create_enc28j60<SPI, PNCS, PRST>(
    delay: &mut SysTick,
    spi: SPI,
    ncs: PNCS,
    mut rst: PRST,
    addr: [u8; 6],
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

    log::debug!("Setting up ENC28J60.");
    let enc28j60 = Enc28j60::new(
        spi,
        ncs,
        enc28j60::Unconnected, // Interrupt
        rst,
        delay,
        enc28j60::BUF_SZ - enc28j60::MAX_FRAME_LENGTH,
        addr,
    );
    log::debug!("ENC28J60 setup done.");
    match enc28j60 {
        Ok(enc) => enc,
        Err(err) => {
            log::warn!("Failed to initialise ENC: {:?}", err);
            halt!();
        }
    }
}

pub struct Enc28j60Phy<D: Driver> {
    rx_buffer: [u8; ENC28J60_MTU],
    tx_buffer: [u8; ENC28J60_MTU],
    driver: D,
}

impl<D: Driver> Enc28j60Phy<D> {
    pub fn new(driver: D) -> Self {
        Self {
            rx_buffer: [0; ENC28J60_MTU],
            tx_buffer: [0; ENC28J60_MTU],
            driver,
        }
    }
}

impl<'a, D: 'a + Driver> phy::Device<'a> for Enc28j60Phy<D> {
    type RxToken = Enc28j60RxToken<'a>;
    type TxToken = Enc28j60TxToken<'a, D>;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = ENC28J60_MTU;
        caps.max_burst_size = Some(1);
        caps.checksum = ChecksumCapabilities::default();
        caps
    }

    fn receive(&'a mut self) -> Option<(Self::RxToken, Self::TxToken)> {
        let pending = self.driver.pending_packets();
        if pending > 0 {
            log::trace!("We have {} pending packets", pending);
            self.driver.receive(&mut self.rx_buffer);

            Some((
                Enc28j60RxToken {
                    buffer: &mut self.rx_buffer,
                },
                Enc28j60TxToken {
                    buffer: &mut self.tx_buffer,
                    driver: &mut self.driver,
                },
            ))
        } else {
            None
        }
    }

    fn transmit(&'a mut self) -> Option<Self::TxToken> {
        Some(Enc28j60TxToken {
            buffer: &mut self.tx_buffer,
            driver: &mut self.driver,
        })
    }
}

pub struct Enc28j60RxToken<'a> {
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

pub struct Enc28j60TxToken<'a, D> {
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
