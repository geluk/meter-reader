pub struct Random {
    state: u32,
}

impl Random {
    pub fn new(seed: u32) -> Self {
        Random { state: seed }
    }

    pub fn next_u32(&mut self) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        self.state
    }

    pub fn next(&mut self, upper_bound: u32) -> u32 {
        let upper_bound = upper_bound;
        loop {
            let rand = self.next_u32();
            let sets = u32::max_value() / upper_bound;
            if rand < sets * upper_bound {
                return rand % upper_bound;
            }
        }
    }
}
