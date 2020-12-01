use core::{cell::RefCell, sync::atomic::{AtomicBool, Ordering}};

use cortex_m::interrupt::{Mutex, free};

use teensy4_bsp::{hal::{uart::UART, ccm, dma, iomuxc::{prelude::consts}}, interrupt};

const DMA_RX_CHANNEL: usize = 7;
const RX_RESERV: usize = 2;
const RX_BUF_SZ: usize = 64;

type DmaPeripheral = dma::Peripheral<UART<consts::U2>, u8, dma::Linear<u8>, dma::Circular<u8>>;

#[repr(align(64))]
struct Align64(dma::Buffer<[u8; RX_BUF_SZ]>);

static RX_MEM: Align64 = Align64(dma::Buffer::new([0; RX_BUF_SZ]));
static RX_BUFFER: Mutex<RefCell<Option<dma::Circular<u8>>>> = Mutex::new(RefCell::new(None));

static mut DMA_PERIPHERAL: Option<DmaPeripheral> = None;

static RX_READY: AtomicBool = AtomicBool::new(false);

pub fn init(uart: UART<consts::U2>, dma: dma::Unclocked, ccm: &mut ccm::Handle) {
    let mut channels = dma.clock(ccm);
    let mut rx_channel = channels[DMA_RX_CHANNEL].take().unwrap();

    rx_channel.set_interrupt_on_completion(true);
    
    let dma_uart = unsafe {
        DMA_PERIPHERAL = Some(dma::Peripheral::new_receive(uart, rx_channel));
        cortex_m::peripheral::NVIC::unmask(interrupt::DMA7_DMA23);
        DMA_PERIPHERAL.as_mut().unwrap()
    };
    let rx_buffer = match dma::Circular::new(&RX_MEM.0) {
        Ok(circular) => circular,
        Err(error) => {
            log::error!("Unable to create circular RX buffer: {:?}", error);
            halt!();
        }
    };
    free(|cs| {
        *RX_BUFFER.borrow(cs).borrow_mut() = Some(rx_buffer);
    });

    let mut rx_buffer =
        free(|cs| RX_BUFFER.borrow(cs).borrow_mut().take()).unwrap_or_else(|| {
            log::error!("RX buffer was not set");
            halt!();
        });
    rx_buffer.reserve(RX_RESERV);
    if let Err(err) = dma_uart.start_receive(rx_buffer) {
        log::error!("Error scheduling DMA receive: {:?}", err);
        halt!();
    }
    RX_READY.store(false, Ordering::Release);
}

pub fn poll(process_buffer: &mut [u8], pos: &mut usize) {
    if RX_READY.load(Ordering::Acquire) {
        RX_READY.store(false, Ordering::Release);
        let mut rx_buffer = free(|cs| RX_BUFFER.borrow(cs).borrow_mut().take())
        .unwrap_or_else(|| {
            log::error!("Failed to acquire RX buffer.");
            halt!();
        });

        let len = rx_buffer.len();
        for i in 0..len {
            process_buffer[i + *pos] = rx_buffer.pop().unwrap();
        }
        *pos += len;

        let res = free(|_| {
            unsafe {
                DMA_PERIPHERAL.as_mut().unwrap().start_receive(rx_buffer)
            }
        });
        if let Err(err) = res {
            log::error!("Error scheduling DMA receive: {:?}", err);
            halt!();
        }
    }
    if *pos > 0 {
        let b = &process_buffer[0..*pos];
        log::debug!("Got message: {}", core::str::from_utf8(b).unwrap());
        *pos = 0;
    }
}

#[cortex_m_rt::interrupt]
unsafe fn DMA7_DMA23() {
    let uart = DMA_PERIPHERAL.as_mut().unwrap();

    // Safe to create a critical section. This won't be preempted by a higher-priority
    // exception.
    let cs = cortex_m::interrupt::CriticalSection::new();

    if uart.is_receive_interrupt() {
        uart.receive_clear_interrupt();
        let mut rx_buffer = RX_BUFFER.borrow(&cs).borrow_mut();
        let data = uart.receive_complete();
        *rx_buffer = data;
        RX_READY.store(true, Ordering::Release);
    }
}
