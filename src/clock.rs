use teensy4_bsp::hal::{
    ccm::{self, perclk, IPGFrequency},
    gpt::{self, Mode, GPT},
};

pub struct Clock {
    gpt: GPT,
    rollover_count: u32,
}

impl Clock {
    pub fn init(
        perclk: perclk::Multiplexer,
        ipg: IPGFrequency,
        handle: &mut ccm::Handle,
        gpt: gpt::Unclocked,
    ) -> Self {
        let mut clk_cfg =
            perclk.configure(handle, perclk::PODF::DIVIDE_10, perclk::CLKSEL::IPG(ipg));

        let mut gpt = gpt.clock(&mut clk_cfg);
        gpt.set_mode(Mode::FreeRunning);
        gpt.set_enable(true);
        Self {
            gpt,
            rollover_count: 0,
        }
    }

    pub fn millis(&mut self) -> i64 {
        if self.gpt.rollover() {
            self.gpt.clear_rollover();
            self.rollover_count += 1;
        }
        let total_ticks = (self.rollover_count as i64) << 32 | self.gpt.count() as i64;
        total_ticks / 7500
    }
}
