#![no_std]
#![no_main]

mod clock;
mod mqtt;
mod network;
mod panic;
mod random;
mod uart;

use embedded_hal::digital::v1_compat::OldOutputPin;
use hal::ccm::{spi, PLL1};
use mqtt::MqttClient;
use teensy4_bsp::{
    hal::{self, ccm, gpio::GPIO, iomuxc::gpio::Pin},
    t40, usb,
    usb::LoggingConfig,
    SysTick,
};

use crate::{
    clock::Clock,
    hal::gpio::Output,
    network::{
        client::TcpClientStore,
        driver::{create_enc28j60, Enc28j60Phy},
        stack::NetworkStack,
    },
    random::Random,
    uart::DsmrUart,
};

const LOG_LEVEL: log::LevelFilter = log::LevelFilter::Debug;
const SPI_CLOCK_HZ: u32 = 16_000_000;
const DSMR_42_BAUD: u32 = 115200;
const DSMR_INVERTED: bool = false;
const ETH_ADDR: [u8; 6] = [0xEE, 0x00, 0x00, 0x0E, 0x4C, 0xA2];

#[cortex_m_rt::entry]
fn main() -> ! {
    let stack_bot = 0u8;
    // Take control of the peripherals.
    let mut per = teensy4_bsp::Peripherals::take().unwrap();
    let core_per = cortex_m::Peripherals::take().unwrap();
    let mut systick = SysTick::new(core_per.SYST);

    // Enable serial USB logging.
    let usb = hal::ral::usb::USB1::take().unwrap();
    let _ = usb::init(
        usb,
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
    let mut uart = uarts
        .uart2
        .init(pins.p14, pins.p15, DSMR_42_BAUD)
        .unwrap_or_else(|err| {
            log::error!("Failed to configure UART: {:?}", err);
            panic!();
        });
    uart.set_rx_inversion(DSMR_INVERTED);

    // Set SPI clock speed.
    match spi4.set_clock_speed(hal::spi::ClockSpeed(SPI_CLOCK_HZ)) {
        Ok(()) => {
            log::info!("Set SPI clock speed to {} Hz", SPI_CLOCK_HZ);
        }
        Err(err) => {
            log::warn!("Unable to set SPI clock speed: {:?}", err);
        }
    }

    let mut dsmr_uart = DsmrUart::new(uart);

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

    log::info!("Entering main loop");
    loop {
        dsmr_uart.poll();
        network.poll(&mut clock);
        network.poll_client(&mut random, &mut client);
        let (read, res) = dsmr42::parse(dsmr_uart.get_buffer());
        match res {
            Ok(telegram) => {
                log::info!("Got new telegram: {}", telegram.device_id);
                client.queue_telegram(telegram);
            }
            Err(dsmr42::TelegramParseError::Incomplete) => {}
            Err(err) => {
                let buffer = dsmr_uart.get_buffer();
                log::warn!(
                    "Failed to parse telegram ({} bytes): {:?}, buffer: {:?}",
                    buffer.len(),
                    err,
                    core::str::from_utf8(buffer)
                );
                dsmr_uart.clear();
            }
        }
        if read > 0 {
            dsmr_uart.consume(read);
        }
    }

    fn make_output_pin<P: Pin>(pin: P) -> OldOutputPin<GPIO<P, Output>> {
        let mut gpio = GPIO::new(pin).output();
        gpio.set_fast(true);
        OldOutputPin::new(gpio)
    }
}
