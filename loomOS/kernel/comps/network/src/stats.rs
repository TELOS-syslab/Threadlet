pub const MAX_SAMPLES: usize = 1000_000;

pub struct KernelStats {
  pub driver_ts: [u64; MAX_SAMPLES],
  pub app_ts: [u64; MAX_SAMPLES],
  pub latencies: [u64; MAX_SAMPLES],
  pub end_cycle: u64,
  pub start_cycle: u64,
  pub total_req: usize,
}

impl KernelStats {
  pub const fn new() -> Self {
    Self {
      driver_ts: [0 as u64; MAX_SAMPLES],
      app_ts: [0 as u64; MAX_SAMPLES],
      latencies: [0 as u64; MAX_SAMPLES],
      end_cycle: 0,
      start_cycle: 0,
      total_req: 0,
    }
  }

  pub fn start(&mut self, force: bool) {
    if self.start_cycle == 0 || force {
      if self.start_cycle == 0 {
        self.start_cycle = ostd::arch::riscv::read_tsc();
      } else {
        self.start_cycle = 0;
      }
      self.end_cycle = 0;
      self.total_req = 0;
    }
  }

  pub fn record(&mut self, rpc_id: u32) {
    self.driver_ts[rpc_id as usize] = ostd::arch::riscv::read_tsc();
  }

  pub fn add(&mut self, rpc_id: u32) {
    let mut now = ostd::arch::riscv::read_tsc();
    self.app_ts[rpc_id as usize] = now;
    self.latencies[rpc_id as usize] = now - self.driver_ts[rpc_id as usize];
    self.end_cycle = now;
    self.total_req += 1;
  }

  pub fn clear(&mut self) {
    self.end_cycle = 0;
    self.start_cycle = 0;
  }

  pub fn report(&self) {
    ostd::early_println!("[krpc-server] Report");

    if self.total_req == 0 {
      return;
    }
    
    let mut sorted_lats = self.latencies[..self.total_req].to_vec();

    for id in 0..self.total_req {
      ostd::early_println!("%{}-rpc driver {} app {} costs {} us", id, self.driver_ts[id], self.app_ts[id], self.latencies[id]);
    }
    
    sorted_lats.sort_unstable();

    let start = self.start_cycle;
    let end = self.end_cycle;
    
    let elapsed_secs = (end - start) as f64 / 1000_000 as f64;
    let throughput = self.total_req as f64 / elapsed_secs;

    let p99 = sorted_lats[(self.total_req as f64 * 0.99) as usize];

    ostd::early_println!("Total requests: {}", self.total_req);
    ostd::early_println!("Elapsed_secs: {}", elapsed_secs);
    ostd::early_println!("P99 Latency: {} us", p99);
    ostd::early_println!("Total Latency: end {} us, start {} us, cost {} secs", end, start, (end - start) as f64 / 1000_000 as f64);
    ostd::early_println!("Throughput:  {} rps", throughput);
  }
}

pub static mut G_STATS: KernelStats = KernelStats::new();
