package freechips.rocketchip.rocket

import chisel3._
import chisel3.util._
import freechips.rocketchip.tile.{CoreBundle, CoreModule}
import org.chipsalliance.cde.config.Parameters

class ThreadContextOp(implicit p: Parameters) extends CoreBundle()(p) {
  val thread = UInt(threadIdLength.W)
  val slot = UInt(threadSlotIdLength.W)
}

class ThreadContextSaverIO(implicit p: Parameters) extends CoreBundle()(p) {
  val save_start = Input(Valid(new ThreadContextOp))
  val load_start = Input(Valid(new ThreadContextOp))
  val load_redirect = Flipped(Decoupled(new ThreadContextOp))
  val base = Input(UInt(paddrBits.W))
  val base_valid = Input(Bool())
  val rf_raddr = Output(UInt((threadSlotIdLength + 5).W))
  val rf_rdata = Input(UInt(xLen.W))
  val rf_wen = Output(Bool())
  val rf_waddr = Output(UInt((threadSlotIdLength + 5).W))
  val rf_wdata = Output(UInt(xLen.W))
  val rf_wready = Input(Bool())
  val busy_slots = Output(UInt(threadSlotCount.W))
  val save_done = Output(Valid(new ThreadContextOp))
  val load_done = Output(Valid(new ThreadContextOp))
  val cache = new HellaCacheIO
}

class ThreadContextSaver(implicit p: Parameters) extends CoreModule {
  require(xLen == 64, "threadlet context save currently stores 64-bit GPRs only")

  private def threadletPrintf(body: => Unit): Unit =
    if (coreParams.threadletAreaDebugPrintf) { body }

  private def threadletAssert(cond: => Bool, message: String): Unit =
    if (coreParams.threadletAreaDebugAssert) { assert(cond, message) }

  private val regsPerThread = 32
  private val bytesPerReg = 8
  private val bytesPerThread = regsPerThread * bytesPerReg
  private val regIdxBits = log2Ceil(regsPerThread)
  private val maxInflight = 8

  val io = IO(new ThreadContextSaverIO)

  val dcIF = Module(new SimpleHellaCacheIF(maxInflight))
  dcIF.io.cache <> io.cache

  val s_idle :: s_active :: Nil = Enum(2)
  val state = RegInit(s_idle)
  val active_load = RegInit(false.B)
  val active_thread = RegInit(0.U(threadIdLength.W))
  val active_slot = RegInit(0.U(threadSlotIdLength.W))
  val discard_load = RegInit(false.B)
  val redirect_pending = RegInit(false.B)
  val redirect_thread = RegInit(0.U(threadIdLength.W))
  val redirect_slot = RegInit(0.U(threadSlotIdLength.W))
  private val countBits = log2Ceil(regsPerThread + 1)
  val req_count = RegInit(0.U(countBits.W))
  val resp_count = RegInit(0.U(countBits.W))
  val done_count = RegInit(0.U(countBits.W))

  class LoadResp extends Bundle {
    val reg = UInt(regIdxBits.W)
    val data = UInt(xLen.W)
  }
  val loadRespQ = Module(new Queue(new LoadResp, maxInflight))

  val active = state === s_active
  val start_save = (state === s_idle) && io.save_start.valid && io.base_valid
  val start_load = (state === s_idle) && io.load_start.valid && io.base_valid
  val start_any = start_save || start_load
  io.load_redirect.ready := active && active_load && !discard_load &&
    (io.load_redirect.bits.slot === active_slot)
  val redirect_fire = io.load_redirect.fire
  val active_slot_mask = Mux(active, UIntToOH(active_slot, threadSlotCount), 0.U(threadSlotCount.W))
  io.busy_slots := active_slot_mask
  val issue_reg = req_count(regIdxBits - 1, 0)
  val reqs_inflight = req_count - resp_count
  val load_not_written = req_count - done_count
  val pipe_slots_used = Mux(active_load, load_not_written, reqs_inflight)
  val can_issue = active &&
    !discard_load && !redirect_fire &&
    (req_count =/= regsPerThread.U(countBits.W)) &&
    (pipe_slots_used < maxInflight.U(countBits.W))

  io.rf_raddr := Cat(active_slot, issue_reg)
  val dropping_load = active && active_load && (discard_load || redirect_fire)
  io.rf_wen := loadRespQ.io.deq.fire && !dropping_load
  io.rf_waddr := Cat(active_slot, loadRespQ.io.deq.bits.reg)
  io.rf_wdata := Mux(loadRespQ.io.deq.bits.reg === 0.U, 0.U, loadRespQ.io.deq.bits.data)

  dcIF.io.requestor.req.valid := can_issue
  dcIF.io.requestor.req.bits.addr := (io.base +
    (active_thread << log2Ceil(bytesPerThread)).asUInt +
    (issue_reg << log2Ceil(bytesPerReg)).asUInt)(paddrBits - 1, 0)
  dcIF.io.requestor.req.bits.tag := issue_reg
  dcIF.io.requestor.req.bits.cmd := Mux(active_load, M_XRD, M_XWR)
  dcIF.io.requestor.req.bits.size := log2Ceil(bytesPerReg).U
  dcIF.io.requestor.req.bits.signed := false.B
  dcIF.io.requestor.req.bits.dprv := PRV.M.U
  dcIF.io.requestor.req.bits.dv := false.B
  dcIF.io.requestor.req.bits.data := Fill(coreDataBits / xLen, Mux(issue_reg === 0.U, 0.U, io.rf_rdata))
  dcIF.io.requestor.req.bits.phys := true.B
  dcIF.io.requestor.req.bits.no_resp := false.B
  dcIF.io.requestor.req.bits.no_alloc := false.B
  dcIF.io.requestor.req.bits.no_xcpt := true.B
  dcIF.io.requestor.req.bits.mask := ((BigInt(1) << bytesPerReg) - 1).U(coreDataBytes.W)
  dcIF.io.requestor.req.bits.idx.foreach(_ := dcIF.io.requestor.req.bits.addr)
  dcIF.io.requestor.s1_kill := false.B
  dcIF.io.requestor.s2_kill := false.B
  dcIF.io.requestor.s1_data.data := Fill(coreDataBits / xLen, Mux(issue_reg === 0.U, 0.U, io.rf_rdata))
  dcIF.io.requestor.s1_data.mask := ((BigInt(1) << bytesPerReg) - 1).U(coreDataBytes.W)
  dcIF.io.requestor.keep_clock_enabled := active || start_any
  dcIF.io.requestor.dcache_print_enable := false.B
  dcIF.io.requestor.dcache_monitor.valid := false.B
  dcIF.io.requestor.dcache_monitor.bits := 0.U.asTypeOf(dcIF.io.requestor.dcache_monitor.bits)
  dcIF.io.requestor.uncached_resp.foreach(_.ready := true.B)

  loadRespQ.io.enq.valid := active && active_load && !dropping_load && dcIF.io.requestor.resp.valid
  loadRespQ.io.enq.bits.reg := dcIF.io.requestor.resp.bits.tag(regIdxBits - 1, 0)
  loadRespQ.io.enq.bits.data := dcIF.io.requestor.resp.bits.data(xLen - 1, 0)
  loadRespQ.io.deq.ready := active && active_load &&
    Mux(dropping_load, true.B, io.rf_wready)

  io.save_done.valid := false.B
  io.save_done.bits.thread := active_thread
  io.save_done.bits.slot := active_slot
  io.load_done.valid := false.B
  io.load_done.bits.thread := active_thread
  io.load_done.bits.slot := active_slot

  when (state === s_idle) {
    when (start_any) {
      active_load := start_load
      active_thread := Mux(start_save, io.save_start.bits.thread, io.load_start.bits.thread)
      active_slot := Mux(start_save, io.save_start.bits.slot, io.load_start.bits.slot)
      req_count := 0.U
      resp_count := 0.U
      done_count := 0.U
      discard_load := false.B
      redirect_pending := false.B
      state := s_active
    }
  }.otherwise {
    when (redirect_fire) {
      discard_load := true.B
      redirect_pending := true.B
      redirect_thread := io.load_redirect.bits.thread
      redirect_slot := io.load_redirect.bits.slot
    }

    when (dcIF.io.requestor.req.fire) {
      req_count := req_count + 1.U
    }

    when (dcIF.io.requestor.resp.valid) {
      when (active_load) {
        resp_count := resp_count + 1.U
      }.otherwise {
        resp_count := resp_count + 1.U
        done_count := done_count + 1.U
        when (done_count === (regsPerThread - 1).U(countBits.W)) {
          io.save_done.valid := true.B
          threadletPrintf {
            midas.targetutils.SynthesizePrintf(printf(
              "[ThreadContextSaver][save_done] thread=%d slot=%d\n",
              active_thread, active_slot))
          }
          state := s_idle
        }
      }
    }

    when (loadRespQ.io.deq.fire && !dropping_load) {
      done_count := done_count + 1.U
      when (done_count === (regsPerThread - 1).U(countBits.W)) {
        io.load_done.valid := true.B
        threadletPrintf {
          midas.targetutils.SynthesizePrintf(printf(
            "[ThreadContextSaver][load_done] thread=%d slot=%d\n",
            active_thread, active_slot))
        }
        state := s_idle
      }
    }

    when (discard_load && redirect_pending &&
        (resp_count === req_count) && !loadRespQ.io.deq.valid) {
      active_thread := redirect_thread
      active_slot := redirect_slot
      req_count := 0.U
      resp_count := 0.U
      done_count := 0.U
      discard_load := false.B
      redirect_pending := false.B
    }
  }

  threadletAssert(!loadRespQ.io.enq.valid || loadRespQ.io.enq.ready,
    "ThreadContextSaver load response queue overflow")
  threadletAssert(!(start_save && start_load),
    "ThreadContextSaver accepts only one context operation at a time")
}

class ThreadContextDmemArbiter(implicit p: Parameters) extends CoreModule {
  private val localClientBits = 1
  private val localTagBits = coreParams.dcacheReqTagBits - localClientBits
  private val tagWidth = coreParams.dcacheReqTagBits + log2Ceil(dcacheArbPorts)

  require(localTagBits >= threadSlotIdLength + 5 + 1,
    "thread context D$ arbiter needs one spare DCache request tag bit")

  val io = IO(new Bundle {
    val core = Flipped(new HellaCacheIO)
    val saver = Flipped(new HellaCacheIO)
    val mem = new HellaCacheIO
  })

  private def addClientId(tag: UInt, saver: Bool): UInt = {
    val out = Wire(UInt(tagWidth.W))
    out := Cat(saver, tag(localTagBits - 1, 0))
    out
  }

  private def stripClientId(tag: UInt): UInt = {
    val out = Wire(UInt(tagWidth.W))
    out := tag(localTagBits - 1, 0)
    out
  }

  private def isSaverTag(tag: UInt): Bool = tag(localTagBits)

  val s1_saver = RegInit(false.B)
  val s2_saver = RegNext(s1_saver, init = false.B)
  val use_saver = !io.core.req.valid && io.saver.req.valid

  io.mem.keep_clock_enabled := io.core.keep_clock_enabled || io.saver.keep_clock_enabled
  io.mem.dcache_print_enable := io.core.dcache_print_enable
  io.mem.dcache_monitor := io.core.dcache_monitor

  io.mem.req.valid := io.core.req.valid || io.saver.req.valid
  io.mem.req.bits := io.core.req.bits
  when (use_saver) {
    io.mem.req.bits := io.saver.req.bits
  }
  io.mem.req.bits.tag := addClientId(Mux(use_saver, io.saver.req.bits.tag, io.core.req.bits.tag), use_saver)
  io.core.req.ready := io.mem.req.ready
  io.saver.req.ready := io.mem.req.ready && !io.core.req.valid
  when (io.mem.req.fire) {
    s1_saver := use_saver
  }

  io.mem.s1_kill := Mux(s1_saver, io.saver.s1_kill, io.core.s1_kill)
  io.mem.s1_data := io.core.s1_data
  when (s1_saver) {
    io.mem.s1_data := io.saver.s1_data
  }
  io.mem.s2_kill := Mux(s2_saver, io.saver.s2_kill, io.core.s2_kill)

  io.core.resp.valid := io.mem.resp.valid && !isSaverTag(io.mem.resp.bits.tag)
  io.saver.resp.valid := io.mem.resp.valid && isSaverTag(io.mem.resp.bits.tag)
  io.core.resp.bits := io.mem.resp.bits
  io.saver.resp.bits := io.mem.resp.bits
  io.core.resp.bits.tag := stripClientId(io.mem.resp.bits.tag)
  io.saver.resp.bits.tag := stripClientId(io.mem.resp.bits.tag)

  io.core.s2_nack := io.mem.s2_nack && !s2_saver
  io.saver.s2_nack := io.mem.s2_nack && s2_saver
  io.core.s2_nack_cause_raw := io.mem.s2_nack_cause_raw
  io.saver.s2_nack_cause_raw := io.mem.s2_nack_cause_raw
  io.core.s2_uncached := io.mem.s2_uncached
  io.saver.s2_uncached := io.mem.s2_uncached
  io.core.s2_paddr := io.mem.s2_paddr
  io.saver.s2_paddr := io.mem.s2_paddr
  io.core.s2_xcpt := io.mem.s2_xcpt
  io.saver.s2_xcpt := io.mem.s2_xcpt
  io.core.s2_gpa := io.mem.s2_gpa
  io.saver.s2_gpa := io.mem.s2_gpa
  io.core.s2_gpa_is_pte := io.mem.s2_gpa_is_pte
  io.saver.s2_gpa_is_pte := io.mem.s2_gpa_is_pte
  io.core.ordered := io.mem.ordered
  io.saver.ordered := io.mem.ordered
  io.core.store_pending := io.mem.store_pending
  io.saver.store_pending := io.mem.store_pending
  io.core.perf := io.mem.perf
  io.saver.perf := io.mem.perf
  io.core.clock_enabled := io.mem.clock_enabled
  io.saver.clock_enabled := io.mem.clock_enabled
  io.core.dcache_wakeup := io.mem.dcache_wakeup
  io.saver.dcache_wakeup := io.mem.dcache_wakeup
  io.core.dcache_probe := io.mem.dcache_probe
  io.saver.dcache_probe := io.mem.dcache_probe
  io.core.dcache_self_evict := io.mem.dcache_self_evict
  io.saver.dcache_self_evict := io.mem.dcache_self_evict
  io.core.replay_next := io.mem.replay_next
  io.saver.replay_next := io.mem.replay_next

  io.mem.uncached_resp.foreach { memResp =>
    memResp.ready := false.B
    io.core.uncached_resp.foreach { coreResp =>
      val hit = !isSaverTag(memResp.bits.tag)
      coreResp.valid := memResp.valid && hit
      coreResp.bits := memResp.bits
      coreResp.bits.tag := stripClientId(memResp.bits.tag)
      when (coreResp.ready && hit) {
        memResp.ready := true.B
      }
    }
    io.saver.uncached_resp.foreach { saverResp =>
      val hit = isSaverTag(memResp.bits.tag)
      saverResp.valid := memResp.valid && hit
      saverResp.bits := memResp.bits
      saverResp.bits.tag := stripClientId(memResp.bits.tag)
      when (saverResp.ready && hit) {
        memResp.ready := true.B
      }
    }
  }
}
