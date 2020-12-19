use core::cmp;

use embedded_hal::serial::Read;
use teensy4_bsp::hal::{iomuxc::prelude::consts, uart::UART};

pub struct DsmrUart {
    uart: UART<consts::U2>,
    read_buffer: [u8; 1024],
    read_buffer_pos: usize,
}

impl DsmrUart {
    pub fn new(mut uart: UART<consts::U2>) -> Self {
        uart.set_rx_fifo(true);
        Self {
            uart,
            read_buffer: [0; 1024],
            read_buffer_pos: 0,
        }
    }

    pub fn poll(&mut self) {
        loop {
            match self.uart.read() {
                Ok(b) => {
                    self.read_buffer[self.read_buffer_pos] = b;
                    self.read_buffer_pos += 1;
                }
                Err(nb::Error::WouldBlock) => break,
                Err(nb::Error::Other(e)) => {
                    log::warn!("Error during polling: {:?}", e);
                    break;
                },
            }
        }
    }

    pub fn get_buffer(&self) -> &[u8] {
        &self.read_buffer[..self.read_buffer_pos]
    }

    /// Advances the read buffer by `count` bytes.
    pub fn consume(&mut self, count: usize) {
        let count = cmp::min(count, self.read_buffer_pos);
        self.read_buffer.copy_within(count.., 0);

        let prev_len = self.read_buffer_pos;
        self.read_buffer_pos -= count;

        log::info!("Consumed {} of {} bytes", count, prev_len);
    }

    pub fn clear(&mut self) {
        self.read_buffer = [0; 1024];
        self.read_buffer_pos = 0;
    }
}
