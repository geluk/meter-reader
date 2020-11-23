#![allow(deprecated)] // Required because enc28j60 depends on v1.

use core::result::Result;

use embedded_hal::{
    blocking::spi::{transfer, write},
    blocking::spi::{Transfer, Write},
    digital::v1::OutputPin,
};
use enc28j60::Enc28j60;
use smoltcp::{
    phy::{self, ChecksumCapabilities, DeviceCapabilities},
    time::Instant,
};
use teensy4_bsp::SysTick;

const TX_BUF: usize = enc28j60::MAX_FRAME_LENGTH as usize;
const RX_BUF: usize = enc28j60::BUF_SZ as usize - TX_BUF;
const BUF_TOLERANCE: usize = 256;

type DriverError = enc28j60::Error<teensy4_bsp::hal::spi::Error>;
type SpiError = teensy4_bsp::hal::spi::Error;

// This trait isn't meant to be a generic abstraction over any network driver,
// it's just here so we can program our smoltcp glue against a simple trait
// instead of the generic soup resulting from Enc28j60 and its trait bounds.
pub trait Driver: 'static {
    fn pending_packets(&mut self) -> Result<u8, SpiError>;

    fn receive(&mut self, buffer: &mut [u8]) -> Result<u16, SpiError>;

    fn transmit(&mut self, buffer: &[u8]) -> Result<(), DriverError>;
}

impl<SPI, NCS, INT, RESET> Driver for Enc28j60<SPI, NCS, INT, RESET>
where
    SPI: Transfer<u8, Error = SpiError> + Write<u8, Error = SpiError> + 'static,
    NCS: OutputPin + 'static,
    INT: enc28j60::IntPin + 'static,
    RESET: enc28j60::ResetPin + 'static,
{
    #[inline]
    fn pending_packets(&mut self) -> Result<u8, SpiError> {
        Enc28j60::pending_packets(self)
    }

    #[inline]
    fn receive(&mut self, buffer: &mut [u8]) -> Result<u16, SpiError> {
        log::trace!("Requesting next packet from device");
        match Enc28j60::receive(self, buffer) {
            Ok(recv) => {
                log::trace!("Got next packet from device, {} bytes", recv);
                Ok(recv)
            }
            Err(err) => {
                log::warn!("Receive failed: {:?}", err);
                Err(err)
            }
        }
    }

    #[inline]
    fn transmit(&mut self, buffer: &[u8]) -> Result<(), DriverError> {
        log::trace!("Sending {} bytes to device", buffer.len());
        match Enc28j60::transmit(self, buffer) {
            Ok(()) => {
                log::trace!("Sent {} bytes: \n{:02x?}", buffer.len(), buffer);
                Ok(())
            }
            Err(e) => {
                log::warn!("Failed to send {} bytes to device", buffer.len());
                Err(e)
            }
        }
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
    SPI: write::Default<u8, Error = SpiError> + transfer::Default<u8, Error = SpiError>,
    PNCS: OutputPin + 'static,
    PRST: OutputPin + 'static,
{
    log::debug!("Initialising ENC28J60 driver");
    // Ensure the reset pin is high on startup
    rst.set_high();
    delay.delay(1);

    let enc28j60 = Enc28j60::new(
        spi,
        ncs,
        enc28j60::Unconnected, // Interrupt
        rst,
        delay,
        RX_BUF as u16,
        addr,
    );
    match enc28j60 {
        Ok(enc) => {
            delay.delay(100);
            log::debug!("ENC28J60 setup done.");
            enc
        }
        Err(err) => {
            log::warn!("Failed to initialise ENC: {:?}", err);
            halt!();
        }
    }
}

pub struct Enc28j60Phy<D: Driver> {
    rx_buffer: [u8; RX_BUF - BUF_TOLERANCE],
    tx_buffer: [u8; TX_BUF],
    driver: D,
}

impl<D: Driver> Enc28j60Phy<D> {
    pub fn new(driver: D) -> Self {
        Self {
            rx_buffer: [0; RX_BUF - BUF_TOLERANCE],
            tx_buffer: [0; TX_BUF],
            driver,
        }
    }
}

impl<'a, D: 'a + Driver> phy::Device<'a> for Enc28j60Phy<D> {
    type RxToken = Enc28j60RxToken<'a>;
    type TxToken = Enc28j60TxToken<'a, D>;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = TX_BUF;
        caps.max_burst_size = Some(1);
        caps.checksum = ChecksumCapabilities::default();
        caps
    }

    fn receive(&'a mut self) -> Option<(Self::RxToken, Self::TxToken)> {
        let pending = self
            .driver
            .pending_packets()
            .map_err(|e| log::warn!("Failed to retrieve pending packet count: {:?}", e))
            .ok()?;
        if pending > 0 {
            log::trace!("We have {} pending packets", pending);
            self.driver
                .receive(&mut self.rx_buffer)
                .map_err(|e| log::warn!("Failed to receive packet from driver: {:?}", e))
                .ok()?;
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
    fn consume<R, F>(mut self, _timestamp: Instant, f: F) -> smoltcp::Result<R>
    where
        F: FnOnce(&mut [u8]) -> smoltcp::Result<R>,
    {
        f(&mut self.buffer)
    }
}

pub struct Enc28j60TxToken<'a, D> {
    buffer: &'a mut [u8],
    driver: &'a mut D,
}

impl<'a, D: Driver> phy::TxToken for Enc28j60TxToken<'a, D> {
    fn consume<R, F>(self, _timestamp: Instant, len: usize, f: F) -> smoltcp::Result<R>
    where
        F: FnOnce(&mut [u8]) -> smoltcp::Result<R>,
    {
        if len > self.buffer.len() {
            log::warn!(
                "Packet length ({}) exceeds Tx buffer size ({})",
                len,
                self.buffer.len()
            );
            return Err(smoltcp::Error::Exhausted);
        }
        f(&mut self.buffer[..len]).and_then(|r| {
            self.driver.transmit(&self.buffer[..len]).map_err(|e| {
                log::warn!("Transmit error: {:?}", e);
                smoltcp::Error::Illegal
            })?;
            Ok(r)
        })
    }
}
