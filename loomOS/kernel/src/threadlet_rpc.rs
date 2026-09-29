// SPDX-License-Identifier: MPL-2.0

use ostd::arch::riscv::threadlet;
use ostd::cpu::PinCurrentCpu;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use aster_network::stats::*;

const RPC_KEY_SIZE: usize = 10;
const RPC_VALUE_SIZE: usize = 100;
const RPC_KV_BUCKETS: usize = 1024;
const ETH_HEADER_LEN: usize = 14;
const IPV4_MIN_HEADER_LEN: usize = 20;
const UDP_HEADER_LEN: usize = 8;
const MIN_UDP_FRAME_LEN: usize = ETH_HEADER_LEN + IPV4_MIN_HEADER_LEN + UDP_HEADER_LEN;
const RPC_MAGIC: u64 = 0x7777;
const RPC_HEADER_SIZE: usize = 8 + 4 + 4; // magic + id + type
const RPC_REQUEST_WIRE_SIZE: usize = RPC_HEADER_SIZE + RPC_KEY_SIZE + RPC_VALUE_SIZE;
const RPC_WAIT_WIRE_SIZE: usize = RPC_REQUEST_WIRE_SIZE + 8;
const RPC_TYPE_PUT: u32 = 0;
const RPC_TYPE_GET: u32 = 1;
const RPC_TYPE_FINISH: u32 = 2;
const RPC_TYPE_SCAN: u32 = 3;
const RPC_TYPE_WAIT: u32 = 4;
const RPC_WORKER_PROCESS_START: u64 = 10001;
const RPC_WORKER_PROCESS_HALFDONE: u64 = 10002;
const RPC_WORKER_PROCESS_DONE: u64 = 10003;
const RPC_WORKER_PROCESS_WAKEUP: u64 = 10004;
const RPC_WORKER_PROCESS_CHECK: u64 = 10005;
const RPC_WORKER_UDP_START: u64 = 10006;
const RPC_BE_THROUGHPUT: u64 = 21;
pub const RPC_WORKER_THREADLET_PRIO: u32 = 3;
const MAX_RPC_CNT_CPUS: usize = 64;

static RPC_FIRST_PKT_RECORDED: [AtomicBool; MAX_RPC_CNT_CPUS] =
    [const { AtomicBool::new(false) }; MAX_RPC_CNT_CPUS];
static RPC_FIRST_PKT_CNT: [AtomicU64; MAX_RPC_CNT_CPUS] =
    [const { AtomicU64::new(0) }; MAX_RPC_CNT_CPUS];

#[derive(Clone, Copy)]
struct RpcKvEntry {
    valid: bool,
    key: [u8; RPC_KEY_SIZE],
    value: [u8; RPC_VALUE_SIZE],
}

impl RpcKvEntry {
    const fn empty() -> Self {
        Self {
            valid: false,
            key: [0u8; RPC_KEY_SIZE],
            value: [0u8; RPC_VALUE_SIZE],
        }
    }
}

struct RpcKvStore {
    buckets: [RpcKvEntry; RPC_KV_BUCKETS],
}

impl RpcKvStore {
    const fn new() -> Self {
        Self {
            buckets: [RpcKvEntry::empty(); RPC_KV_BUCKETS],
        }
    }

    fn hash(key: &[u8]) -> usize {
        let mut h = 0usize;
        for (i, b) in key.iter().enumerate() {
            h = h.wrapping_add((*b as usize) * i.wrapping_mul(10));
        }
        h % RPC_KV_BUCKETS
    }

    fn put(&mut self, key: &[u8; RPC_KEY_SIZE], value: &[u8; RPC_VALUE_SIZE]) {
        let idx = Self::hash(key);
        self.buckets[idx].valid = true;
        self.buckets[idx].key = *key;
        self.buckets[idx].value = *value;
    }

    fn get(&self, key: &[u8]) -> Option<[u8; RPC_VALUE_SIZE]> {
        let idx = Self::hash(key);
        let ent = self.buckets[idx];
        if ent.valid && ent.key.as_slice() == key {
            Some(ent.value)
        } else {
            None
        }
    }

    fn scan(&self, _key: &[u8], value: &mut [u8; RPC_VALUE_SIZE]) {
        let times = 50;
        for id in 0..times {
            let ent = self.buckets[id];
            value.copy_from_slice(&ent.value);
        }
    }
}

static RPC_KV_STORE: RpcKvStore = RpcKvStore::new();

#[inline(always)]
fn trim_zero_suffix(bytes: &[u8]) -> &[u8] {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    &bytes[..end]
}

#[inline(always)]
fn load_be_u16(buf: &[u8], off: usize) -> Option<u16> {
    let end = off.checked_add(2)?;
    let slice = buf.get(off..end)?;
    Some(u16::from_be_bytes([slice[0], slice[1]]))
}

#[inline(always)]
fn load_be_u32(buf: &[u8], off: usize) -> Option<u32> {
    let end = off.checked_add(4)?;
    let slice = buf.get(off..end)?;
    Some(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

#[inline(always)]
fn load_be_u64(buf: &[u8], off: usize) -> Option<u64> {
    let end = off.checked_add(8)?;
    let slice = buf.get(off..end)?;
    Some(u64::from_be_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
}

#[inline(always)]
fn process_wait_request(run_ns: u64) {
    for _ in 0..run_ns {
        #[allow(unsafe_code)]
        unsafe {
            core::arch::asm!("nop", options(nomem, nostack));
        }
    }
}

const PROCESS_STAGE: u64 = 8;
const RPC_FINISH_STAGE: u64 = 9;
#[inline(always)]
fn process_request(packet: &[u8], cpu_id: usize) {
    threadlet::threadlet_syn_print(PROCESS_STAGE, RPC_WORKER_UDP_START);
    if packet.len() < MIN_UDP_FRAME_LEN {
        return;
    }
    if packet[12] != 0x08 || packet[13] != 0x00 {
        return;
    }

    let ip_off = ETH_HEADER_LEN;
    let Some(&vihl) = packet.get(ip_off) else {
        return;
    };
    if (vihl >> 4) != 4 {
        return;
    }
    let ihl = ((vihl & 0x0f) as usize) << 2;
    if ihl < IPV4_MIN_HEADER_LEN {
        return;
    }

    let Some(udp_off) = ip_off.checked_add(ihl) else {
        return;
    };
    let Some(udp_hdr_end) = udp_off.checked_add(UDP_HEADER_LEN) else {
        return;
    };
    if udp_hdr_end > packet.len() {
        return;
    }
    if packet.get(ip_off + 9).copied() != Some(17) {
        return;
    }

    let Some(ip_total_len) = load_be_u16(packet, ip_off + 2).map(|v| v as usize) else {
        return;
    };
    if ip_total_len < ihl + UDP_HEADER_LEN {
        return;
    }
    let Some(ip_end) = ip_off.checked_add(ip_total_len) else {
        return;
    };
    if ip_end > packet.len() {
        return;
    }

    let Some(udp_len) = load_be_u16(packet, udp_off + 4).map(|v| v as usize) else {
        return;
    };
    if udp_len < UDP_HEADER_LEN {
        return;
    }
    let Some(udp_end) = udp_off.checked_add(udp_len) else {
        return;
    };
    if udp_end > ip_end || udp_end > packet.len() {
        return;
    }

    let Some(payload) = packet.get(udp_off + UDP_HEADER_LEN..udp_end) else {
        return;
    };
    if payload.len() < RPC_HEADER_SIZE {
        return;
    }
    if load_be_u64(payload, 0) != Some(RPC_MAGIC) {
        return;
    }

    let Some(request_id) = load_be_u32(payload, 8) else {
        return;
    };
    let Some(rpc_type) = load_be_u32(payload, 12) else {
        return;
    };

    // unsafe {
    //     let stats_ptr = &G_STATS as *const _ as *mut KernelStats;
    //     (*stats_ptr).record(request_id);
    //     (*stats_ptr).start(false);
    // }
    threadlet::threadlet_syn_print(PROCESS_STAGE, request_id as u64);

    let cnt = crate::ap_idle_nop_loop_cnt_read(cpu_id);
    threadlet::threadlet_syn_print(RPC_BE_THROUGHPUT, cnt);

    match rpc_type {
        RPC_TYPE_WAIT => {
            let Some(run_ns) = load_be_u64(payload, RPC_REQUEST_WIRE_SIZE) else {
                return;
            };
            process_wait_request(run_ns);
            threadlet::threadlet_syn_print(RPC_FINISH_STAGE, request_id as u64);
            if cfg!(polldebug) {
                ostd::early_println!(
                    "[kvrpc-worker] id={} type={} run_ns={}",
                    request_id,
                    rpc_type,
                    run_ns
                );
            }
        }
        RPC_TYPE_GET => {
            let Some(key) = payload.get(RPC_HEADER_SIZE..RPC_HEADER_SIZE + RPC_KEY_SIZE) else {
                return;
            };
            let _ = core::hint::black_box(RPC_KV_STORE.get(key));
            threadlet::threadlet_syn_print(RPC_FINISH_STAGE, request_id as u64);
            if cfg!(polldebug) {
                let key_bytes = trim_zero_suffix(key);
                let key_str = core::str::from_utf8(key_bytes).unwrap_or("<non-utf8>");
                ostd::early_println!(
                    "[kvrpc-worker] id={} type={} key='{}'",
                    request_id,
                    rpc_type,
                    key_str
                );
            }
        }
        RPC_TYPE_SCAN => {
            let Some(key) = payload.get(RPC_HEADER_SIZE..RPC_HEADER_SIZE + RPC_KEY_SIZE) else {
                return;
            };
            let mut tmp_value = [0u8; RPC_VALUE_SIZE];
            RPC_KV_STORE.scan(key, &mut tmp_value);
            let _ = core::hint::black_box(tmp_value);
            threadlet::threadlet_syn_print(RPC_FINISH_STAGE, request_id as u64);
            if cfg!(polldebug) {
                let key_bytes = trim_zero_suffix(key);
                let key_str = core::str::from_utf8(key_bytes).unwrap_or("<non-utf8>");
                ostd::early_println!(
                    "[kvrpc-worker] id={} type={} key='{}'",
                    request_id,
                    rpc_type,
                    key_str
                );
            }
        }
        RPC_TYPE_FINISH => {
            // let tid:u32 = threadlet::threadlet_current() as u32;
            // threadlet::threadlet_set_priority(tid, RPC_WORKER_THREADLET_PRIO+1);
            // unsafe {
            //     let stats_ptr = &G_STATS as *const _ as *mut KernelStats;
            //     (*stats_ptr).report();
            //     (*stats_ptr).start(true);
            // }
            // threadlet::threadlet_set_priority(tid, RPC_WORKER_THREADLET_PRIO);
            if cfg!(polldebug) {
                ostd::early_println!("[kvrpc-worker] id={} type={}", request_id, rpc_type);
            }
            // read the cnt again here, and calculate the diff.
        }
        _ => return,
    }


}

pub fn worker_threadlet() {
    let worker_idx = threadlet::threadlet_get_a0() as usize;
    let preempt_guard = ostd::task::disable_preempt();
    let cpu_id = preempt_guard.current_cpu().as_usize();
    drop(preempt_guard);
    let head_vaddr = aster_network::icenet::rpc_worker_head_vaddr(worker_idx);
    let head_paddr =
        crate::kernel_vaddr_to_paddr(head_vaddr).expect("rpc worker head vaddr->paddr failed");

    loop {
        while aster_network::icenet::rpc_worker_try_process_one(worker_idx, |packet| {
            threadlet::threadlet_syn_print(PROCESS_STAGE, RPC_WORKER_PROCESS_START);
            process_request(packet, cpu_id);
            threadlet::threadlet_syn_print(PROCESS_STAGE, RPC_WORKER_PROCESS_DONE);
        }) {
            aster_network::icenet::rpc_worker_finish_one(worker_idx);
            threadlet::threadlet_pass();
        }

        // Re-arm sleep after draining current ring entries.
        loop{
            threadlet::threadlet_dcache_monitor_set(head_paddr, 0);
            threadlet::threadlet_syn_print(PROCESS_STAGE, RPC_WORKER_PROCESS_CHECK);
            let ring_empty = aster_network::icenet::rpc_worker_ring_empty(worker_idx);
            if ring_empty {
                threadlet::threadlet_yield(); //when waking up, the monitor is cleared.
                threadlet::threadlet_syn_print(PROCESS_STAGE, RPC_WORKER_PROCESS_WAKEUP);
                continue;
            }
            threadlet::threadlet_dcache_monitor_clear(head_paddr, 0);
            break;
        }
    }
}



// per core scheudler for rpc
// Waked up before a threadlet start processing a packet
pub fn scheduler_threadlet() {



    loop{

        //read the worker slots(head,tail) to find out their status(idle or working)
        // each worker threadlet should have a atomic value meaning the rpc_id it is processing
        // if all workers in this core are idle. 
        // {
        //     threadlet::threadlet_yield();
        //     continue;
        // }

        // for worker threadlet this is not is processing a packet, record its rpc_id
        // find a worker threadlet that is disposing a packet for a long time (its current rpc_id == last rpc_id)
        // increase its timeslice


        //cnt = 0;
        // loop SCHEDUELR_IDLE_TIMES  here, meaning that the scheduler threadlet will only trigger a check every n time
        // {
        //     cnt += 1;
        //     if (cnt==SCHEDUELR_IDLE_TIMES) break;
        //     threadlet::threadlet_pass();
        // }


            
    }

}
