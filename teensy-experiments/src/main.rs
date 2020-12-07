#![no_std]
#![no_main]

mod clock;
#[macro_use]
mod macros;
mod mqtt;
mod network;
mod random;
mod uart;

#[cfg(not(test))]
extern crate panic_halt;

use crate::{
    clock::Clock,
    hal::gpio::Output,
    network::{
        client::TcpClientStore,
        driver::{create_enc28j60, Enc28j60Phy},
        stack::NetworkStack,
    },
    random::Random,
};

use embedded_hal::digital::v1_compat::OldOutputPin;
use hal::ccm::{spi, PLL1};
use mqtt::MqttClient;
use teensy4_bsp::{
    hal::{self, ccm, gpio::GPIO, iomuxc::gpio::Pin},
    t40, usb,
    usb::LoggingConfig,
    SysTick,
};
use uart::DmaUart;

const LOG_LEVEL: log::LevelFilter = log::LevelFilter::Debug;
const SPI_BAUD_RATE_HZ: u32 = 16_000_000;
const DSMR_42_BAUD: u32 = 115200;
const ETH_ADDR: [u8; 6] = [0xEE, 0x00, 0x00, 0x0E, 0x4C, 0xA2];

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
        ccm::uart::ClockSelect::OSC,
        ccm::uart::PrescalarSelect::DIVIDE_1,
    );

    let pins = t40::into_pins(per.iomuxc);

    // Set SPI pin assignments.
    let mut spi4 = spi4_builder.build(pins.p11, pins.p12, pins.p13);
    // SET UART pin assignments.
    let uart = uarts
        .uart2
        .init(pins.p14, pins.p15, DSMR_42_BAUD)
        .unwrap_or_else(|err| {
            log::error!("Failed to configure UART: {:?}", err);
            halt!();
        });

    // Set SPI clock speed.
    match spi4.set_clock_speed(hal::spi::ClockSpeed(SPI_BAUD_RATE_HZ)) {
        Ok(()) => {
            log::info!("Set SPI clock speed to {} Hz", SPI_BAUD_RATE_HZ);
        }
        Err(err) => {
            log::warn!("Unable to set SPI clock speed: {:?}", err);
        }
    }

    let mut dma_uart = DmaUart::new(uart, per.dma, &mut per.ccm.handle);

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
        dma_uart.poll();
        network.poll(&mut clock, &mut random);
        network.poll_client(&mut random, &mut client);
        let (read, res) = dsmr42::parse(&dma_uart.get_buffer());
        match res {
            Ok(telegram) => {
                log::info!("Got new telegram: {:#?}", telegram);
            }
            Err(err) => {
                log::warn!("Failed to parse telegram: {:?}", err);
            }
        }
        dma_uart.consume(read);
    }

    fn make_output_pin<P: Pin>(pin: P) -> OldOutputPin<GPIO<P, Output>> {
        let mut gpio = GPIO::new(pin).output();
        gpio.set_fast(true);
        OldOutputPin::new(gpio)
    }
}
