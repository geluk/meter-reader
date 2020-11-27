#![no_std]
#![no_main]

mod clock;
#[macro_use]
mod macros;
mod mqtt;
mod network;
mod random;

extern crate panic_halt;

use embedded_hal::prelude::_embedded_hal_serial_Read;
use core::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{
    clock::Clock,
    hal::{gpio::Output, iomuxc::consts::U2},
    network::{
        client::TcpClientStore,
        driver::{create_enc28j60, Enc28j60Phy},
        stack::NetworkStack,
    },
    random::Random,
};
use cortex_m::interrupt::{free, Mutex};

use embedded_hal::digital::v1_compat::OldOutputPin;
use hal::ccm::{spi, uart, PLL1};
use mqtt::MqttClient;
use teensy4_bsp::{
    hal::{self, dma, gpio::GPIO, iomuxc::gpio::Pin, uart::UART},
    interrupt, t40, usb,
    usb::LoggingConfig,
    SysTick,
};

const LOG_LEVEL: log::LevelFilter = log::LevelFilter::Debug;
const SPI_BAUD_RATE_HZ: u32 = 16_000_000;
const ETH_ADDR: [u8; 6] = [0xEE, 0x00, 0x00, 0x0E, 0x4C, 0xA2];
const DSMR_42_BAUD: u32 = 115200;
const DMA_RX_CHANNEL: usize = 7;
const RX_RESERV: usize = 2;
const RX_BUF_SZ: usize = 64;

#[repr(align(64))]
struct Align64(dma::Buffer<[u8; RX_BUF_SZ]>);

static RX_MEM: Align64 = Align64(dma::Buffer::new([0; RX_BUF_SZ]));
static RX_BUFFER: Mutex<RefCell<Option<dma::Circular<u8>>>> = Mutex::new(RefCell::new(None));

type DmaUart = dma::Peripheral<UART<U2>, u8, dma::Linear<u8>, dma::Circular<u8>>;
static mut DMA_PERIPHERAL: Option<DmaUart> = None;

static RX_READY: AtomicBool = AtomicBool::new(false);

#[cortex_m_rt::entry]
fn main() -> ! {
    let stack_bot = 0u8;
    // Take control of the peripherals.
    let mut per = teensy4_bsp::Peripherals::take().unwrap();
    let core_per = cortex_m::Peripherals::take().unwrap();

    // Enable serial USB logging.
    let mut systick = SysTick::new(core_per.SYST);
    let _ = usb::init(
        &systick,
        LoggingConfig {
            max_level: LOG_LEVEL,
            filters: &[],
        },
    )
    .unwrap();

    // Wait a bit for the host to catch up.
    systick.delay(5000);
    log::info!("USB logging initialised");

    // Set the default clock speed (600MHz).
    let (_, ipg) = per
        .ccm
        .pll1
        .set_arm_clock(PLL1::ARM_HZ, &mut per.ccm.handle, &mut per.dcdc);
    let mut clock = Clock::init(per.ccm.perclk, ipg, &mut per.ccm.handle, per.gpt2);

    // Configure the SPI clock. All SPI builders must be extracted at once,
    // so we discard the ones we don't need.
    let (_, _, _, spi4_builder) = per.spi.clock(
        &mut per.ccm.handle,
        spi::ClockSelect::Pll2,
        spi::PrescalarSelect::LPSPI_PODF_5,
    );

    // Configure UART.
    let uarts = per.uart.clock(
        &mut per.ccm.handle,
        uart::ClockSelect::OSC,
        uart::PrescalarSelect::DIVIDE_1,
    );

    let pins = t40::into_pins(per.iomuxc);

    // Set SPI pin assignments.
    let mut spi4 = spi4_builder.build(pins.p11, pins.p12, pins.p13);
    // SET UART pin assignments.
    let mut uart = uarts
        .uart2
        .init(pins.p14, pins.p15, DSMR_42_BAUD)
        .unwrap_or_else(|err| {
            log::error!("Failed to configure UART: {:?}", err);
            halt!();
        });

    let mut channels = per.dma.clock(&mut per.ccm.handle);
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

    {
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

    let mut process_buffer = [0u8; 1024];
    let mut pos = 0usize;

    loop {
        if RX_READY.load(Ordering::Acquire) {
            RX_READY.store(false, Ordering::Release);
            let mut rx_buffer = free(|cs| RX_BUFFER.borrow(cs).borrow_mut().take())
            .unwrap_or_else(|| {
                log::error!("Failed to acquire RX buffer.");
                halt!();
            });

            let len = rx_buffer.len();
            for i in 0..len {
                process_buffer[i + pos] = rx_buffer.pop().unwrap();
            }
            pos += len;

            //free(|cs| *RX_BUFFER.borrow(cs).borrow_mut() = Some(rx_buffer));
            if let Err(err) = dma_uart.start_receive(rx_buffer) {
                log::error!("Error scheduling DMA receive: {:?}", err);
                halt!();
            }
        } if pos > 0 {
            let b = &process_buffer[0..pos];
            log::debug!("Got message: {}", core::str::from_utf8(b).unwrap());
            pos = 0;
        }
        //log::debug!("Waiting 1000ms");
        //systick.delay(1000);
    }

    // Set SPI clock speed.
    match spi4.set_clock_speed(hal::spi::ClockSpeed(SPI_BAUD_RATE_HZ)) {
        Ok(()) => {
            log::info!("Set SPI clock speed to {} Hz", SPI_BAUD_RATE_HZ);
        }
        Err(err) => {
            log::warn!("Unable to set SPI clock speed: {:?}", err);
        }
    }

    let ncs = make_output_pin(pins.p10);
    let rst = make_output_pin(pins.p9);
    let driver = create_enc28j60(&mut systick, spi4, ncs, rst, ETH_ADDR);
    let mut random = Random::new(clock.ticks());
    let mut store = network::BackingStore::new();

    let mut network = NetworkStack::new(driver, &mut clock, &mut store, ETH_ADDR);

    let mut client_store = TcpClientStore::new();
    let mut client = MqttClient::new();

    network.add_client(&mut client, &mut client_store);

    let stack_top = 0u8;
    log::info!("STACK_BOT: {:p}", &stack_bot);
    log::info!("STACK_TOP: {:p}", &stack_top);
    let stack_bot_addr = (&stack_bot as *const u8) as usize;
    let stack_top_addr = (&stack_top as *const u8) as usize;
    log::info!("STACK_SZE: {}K", (stack_top_addr - stack_bot_addr) / 1024);

    loop {
        client.do_work();
        network.poll(&mut clock, &mut random);
        network.poll_client(&mut random, &mut client);
    }

    fn make_output_pin<P: Pin>(pin: P) -> OldOutputPin<GPIO<P, Output>> {
        let mut gpio = GPIO::new(pin).output();
        gpio.set_fast(true);
        OldOutputPin::new(gpio)
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
