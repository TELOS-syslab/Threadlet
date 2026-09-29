package freechips.rocketchip.rocket

import chisel3._
import chisel3.util._
import freechips.rocketchip.tile.{CoreBundle, CoreModule, BaseTile}
import freechips.rocketchip.subsystem.CacheBlockBytes

import org.chipsalliance.cde.config._

class ThreadCtrl(implicit p: Parameters) extends CoreBundle()(p) {
  val init = Bool()
  val create = Bool()
  val halt = Bool()
  val yields = Bool()
  val pass = Bool()
  val set_prior = Bool()
  val set_slice = Bool()
  val set_deadline = Bool()
  val wakeup = Bool()
  val syn_print = Bool()
  val set_base = Bool()
  val eret = Bool()

  // DCache monitor programming (handled by ThreadManager).
  val dcache_monitor_set = Bool()
  val dcache_monitor_clear = Bool()
  val dcache_monitor_addr = UInt(paddrBits.W)
  
  val create_pc = UInt(vaddrBitsExtended.W)
  val thread = UInt(threadIdLength.W)
  val prior = UInt(priorBits.W)
  val prior_thread = UInt(threadIdLength.W)
  val slice = UInt(xLen.W)
  val slice_thread = UInt(threadIdLength.W)
  val deadline = UInt(xLen.W)
  val deadline_thread = UInt(threadIdLength.W)
  val wakeup_thread = UInt(threadIdLength.W)
  val syn_stage = UInt(64.W)
  val syn_data = UInt(64.W)
  val interrupt_base = UInt(vaddrBitsExtended.W)
}

class ThreadInputInfo(implicit p: Parameters) extends CoreBundle()(p) {
  val pc = UInt(vaddrBitsExtended.W)
  val thread = UInt(threadIdLength.W)
}

class ThreadOutputInfo(implicit p: Parameters) extends CoreBundle()(p) {
  val pc = UInt(vaddrBitsExtended.W)
  val thread = UInt(threadIdLength.W)
  val prior = UInt(priorBits.W)
  val status = UInt(2.W)
}

class ThreadSchedMeta(implicit p: Parameters) extends CoreBundle()(p) {
  val valid = Bool()
  val runable = Bool()
  val priority = UInt(priorBits.W)
  // deadline is a scheduler-tick count (paper tick semantics); width must match
  // ThreadManager.deadlineBits.
  val deadline = UInt(16.W)
  val slice = UInt(16.W)
}

object ThreadTptReadKind {
  final val scan = 0
  final val revalidate = 1
  final val control = 2
}

class ThreadTptReadReq(implicit p: Parameters) extends CoreBundle()(p) {
  val kind = UInt(2.W)
  val thread = UInt(threadIdLength.W)
  val epoch = UInt(8.W)
}

class ThreadTptReadResp(implicit p: Parameters) extends CoreBundle()(p) {
  val req = new ThreadTptReadReq
  val meta = new ThreadSchedMeta
}

class ThreadRankChange(implicit p: Parameters) extends CoreBundle()(p) {
  val thread = UInt(threadIdLength.W)
  val meta = new ThreadSchedMeta
  val rerank = Bool()
}

class ThreadMetadataRead(implicit p: Parameters) extends CoreBundle()(p) {
  val thread = UInt(threadIdLength.W)
  val rerank = Bool()
}

class ThreadSlotLoadCommit(implicit p: Parameters) extends CoreBundle()(p) {
  val thread = UInt(threadIdLength.W)
  val slot = UInt(threadSlotIdLength.W)
  val meta = new ThreadSchedMeta
}

class ThreadXcpt(implicit p: Parameters) extends CoreBundle()(p) {
  val thread = UInt(threadIdLength.W)
  val wb_xcpt = Bool()
  val eret = Bool()
}

class ThreadIO(implicit p: Parameters) extends CoreBundle()(p) {
  val hartid = Input(UInt(hartIdLen.W))
  val ctrl = Flipped(Decoupled(new ThreadCtrl))
  val ret = Decoupled(new ThreadOutputInfo)
  val mem_info = Flipped(Valid(new ThreadInputInfo))
  val wb_info = Flipped(Valid(new ThreadInputInfo))
  val mem_req = Input(UInt(threadIdLength.W))
  val mem_pc = Output(UInt(vaddrBitsExtended.W))
  val xcpt = Flipped(Valid(new ThreadXcpt))
  val new_thread_issue_req = Decoupled(new ThreadOutputInfo)
  val interrupt = Input(Bool())
  val slot_thread = Input(Vec(threadSlotCount, UInt(threadIdLength.W)))
  val slot_sched_blocked = Input(UInt(threadSlotCount.W))
  val slot_load_request = Flipped(Valid(new ThreadContextOp))
  val slot_load_commit = Valid(new ThreadSlotLoadCommit)
  val tpt_read_req = Flipped(Decoupled(new ThreadTptReadReq))
  val tpt_read_resp = Valid(new ThreadTptReadResp)
  val tpt_ready_chunks =
    Output(Vec((threadSupport + 63) / 64, UInt(64.W)))
  val slot_meta = Output(Vec(threadSlotCount, new ThreadSchedMeta))
  val current_thread_out = Output(UInt(threadIdLength.W))
  val rank_change = Decoupled(new ThreadRankChange)
  val metadata_pending = Output(Bool())
  val dcache_wakeup = Input(Valid(UInt(threadIdLength.W)))
  val dcache_probe = Input(Valid(UInt(paddrBits.W)))
  val dcache_self_evict = Input(Valid(UInt(paddrBits.W)))
}


class ThreadManager()(implicit p: Parameters) extends CoreModule {
  def exception_prior = 30
  def interrupt_prior = 31
    // only for interrupt
  def interrupt_thread = 1

  private def rrDistanceAfter(base: UInt, tid: UInt): UInt = {
    val distBits = threadIdLength + 1
    val tidExt = Cat(0.U(1.W), tid)
    val baseExt = Cat(0.U(1.W), base)
    val forward = tidExt - baseExt
    val wrapped = tidExt + threadSupport.U(distBits.W) - baseExt
    Mux(tid > base, forward, wrapped)(distBits - 1, 0)
  }

  private def slotPickAfter(base: UInt, eligible: UInt, slotThread: Vec[UInt]): (Bool, UInt) = {
    val distBits = threadIdLength + 1
    val maxDist = ((BigInt(1) << distBits) - 1).U(distBits.W)
    val rrKeys = (0 until threadSlotCount).map { s =>
      Mux(eligible(s), rrDistanceAfter(base, slotThread(s)), maxDist)
    }
    val minKey = reduceTreeMin(rrKeys)
    val minMask = VecInit((0 until threadSlotCount).map(s =>
      eligible(s) && (rrKeys(s) === minKey))).asUInt
    val pickSlot = if (threadSlotCount == 1) 0.U(threadSlotIdLength.W) else PriorityEncoder(minMask)
    (eligible.orR, slotThread(pickSlot))
  }

  // Balanced reduction helpers to reduce combinational depth on selection logic.
  private def reduceTreeMin(values: Seq[UInt]): UInt = {
    require(values.nonEmpty)
    if (values.length == 1) values.head
    else {
      val next = values.grouped(2).map {
        case Seq(a, b) => Mux(a <= b, a, b)
        case Seq(a) => a
      }.toSeq
      reduceTreeMin(next)
    }
  }

  private def reduceTreeMax(values: Seq[UInt]): UInt = {
    require(values.nonEmpty)
    if (values.length == 1) values.head
    else {
      val next = values.grouped(2).map {
        case Seq(a, b) => Mux(a >= b, a, b)
        case Seq(a) => a
      }.toSeq
      reduceTreeMax(next)
    }
  }

  val io = IO(new ThreadIO) 

  val has_init = RegInit(false.B)
  val random_ctx_time = RegInit(0.U(5.W))
  val current_thread = RegInit(0.U(threadIdLength.W))
  val ctx_switch_enable = RegInit(false.B)
  val pre_prior = RegInit(0.U(5.W))
  val layer_xcpt = RegInit(0.U(5.W))

  random_ctx_time := random_ctx_time + 1.U
  val sched_tick_write_conflict = io.ctrl.fire &&
    (io.ctrl.bits.create || io.ctrl.bits.set_deadline)
  val sched_tick_fire =
    (random_ctx_time === 0.U) && ctx_switch_enable && !sched_tick_write_conflict
  when ((random_ctx_time === 0.U) && ctx_switch_enable && sched_tick_write_conflict) {
    random_ctx_time := 0.U
  }

  private val pcBankCount = 8
  private val pcBankBits = log2Ceil(pcBankCount)
  private val pcRows = threadSupport / pcBankCount
  private val pcRowBits = log2Ceil(pcRows max 2)
  require(threadSupport % pcBankCount == 0,
    "ThreadManager cold PC storage requires complete eight-thread rows")
  private val coldPcBanks =
    Seq.fill(pcBankCount)(SyncReadMem(pcRows, UInt(vaddrBitsExtended.W)))
  val interrupt_base = RegInit(0.U(vaddrBitsExtended.W))
  val eret = RegInit(false.B)

  // Per-threadlet probe monitor table.
  // Each threadlet can monitor at most one cacheline. Only threads with id
  // < monitorThreadCount are monitorable; monitor set/clear on higher ids is a
  // no-op (software rechecks the condition, so a missed arm cannot corrupt).
  private val monitorThreadCount = 64
  require(threadSupport >= monitorThreadCount,
    "ThreadManager monitor table assumes at least monitorThreadCount logical threadlets")
  private val blockOffBits = log2Ceil(p(CacheBlockBytes))
  private val monitorLineBits = (paddrBits - blockOffBits) max 1
  private val monitorScanBanks = 8
  private val monitorBankBits = log2Ceil(monitorScanBanks)
  private val monitorIdBits = log2Ceil(monitorThreadCount max 2)
  private val monitorScanRows = monitorThreadCount / monitorScanBanks
  private val monitorRowBits = log2Ceil(monitorScanRows max 2)
  require(monitorThreadCount % monitorScanBanks == 0,
    "ThreadManager monitor walker requires complete eight-thread rows")
  private val mon_valid = RegInit(VecInit(Seq.fill(monitorThreadCount)(false.B)))
  // Invalid entries gate reads, so the address RAM does not need reset or clear writes.
  private val monitorLineBanks =
    Seq.fill(monitorScanBanks)(Mem(monitorScanRows, UInt(monitorLineBits.W)))
  private val mon_pending = RegInit(VecInit(Seq.fill(monitorThreadCount)(false.B)))

  private class MonitorEvent extends Bundle {
    val line = UInt(monitorLineBits.W)
  }

  private def monitorSupported(thread: UInt): Bool =
    thread < monitorThreadCount.U

  private def monitorIndex(thread: UInt): UInt =
    thread(monitorIdBits - 1, 0)

  private def monitorValid(thread: UInt): Bool =
    Mux(monitorSupported(thread), mon_valid(monitorIndex(thread)), false.B)

  private def monitorPending(thread: UInt): Bool =
    Mux(monitorSupported(thread), mon_pending(monitorIndex(thread)), false.B)

  // @valid: the entry has been set but not runable
  // @runable: can be run on pipeline
  private val sliceTickShift = 5 // 2^5 = 32 cycles
  private val sliceCntBits = 16
  private val sliceMaxTicks = (BigInt(1) << sliceCntBits) - 1
  private val slice_left_ticks = RegInit(1.U(sliceCntBits.W))
  val slice_will_expire = (slice_left_ticks === 0.U) || (slice_left_ticks === 1.U)

  // Direct scheduling key. Zero means no deadline is set.
  // Tick-count width (paper semantics); must match ThreadSchedMeta.deadline.
  private val deadlineBits = 16
  private val initialTptValid =
    ((BigInt(1) << 0) | (BigInt(1) << interrupt_thread)).U(threadSupport.W)
  private val initialTptRunable = (BigInt(1) << 0).U(threadSupport.W)
  val tpt_valid_bits = RegInit(initialTptValid)
  val tpt_runable_bits = RegInit(initialTptRunable)
  private val tptPriorityMem = SyncReadMem(threadSupport, UInt(priorBits.W))
  private val tptSliceMem = SyncReadMem(threadSupport, UInt(sliceCntBits.W))
  private val tptDeadlineMem = SyncReadMem(threadSupport, UInt(deadlineBits.W))

  private def tptBit(bitmap: UInt, thread: UInt): Bool =
    VecInit(bitmap.asBools)(thread)

  private def tptValid(thread: UInt): Bool =
    tptBit(tpt_valid_bits, thread)

  private def tptRunable(thread: UInt): Bool =
    tptBit(tpt_runable_bits, thread)

  private def zeroSchedMeta: ThreadSchedMeta =
    0.U.asTypeOf(new ThreadSchedMeta)

  val slot_meta = RegInit(VecInit(Seq.fill(threadSlotCount)(zeroSchedMeta)))
  val slot_pc = RegInit(VecInit(Seq.fill(threadSlotCount)(0.U(vaddrBitsExtended.W))))

  // NOTE (Phase 3.6 correctness fix): do NOT memoize this across call sites by
  // Chisel node identity. io.ctrl.bits.thread/current_thread are the same node
  // object referenced from many different, non-nested `when` scopes (halt vs
  // yields vs pass vs set_deadline vs xcpt, etc.); reusing a hits vector built
  // inside one `when` block from a sibling/outer scope is invalid Chisel
  // ("operand has escaped the scope of the when in which it was constructed").
  // Each call must construct its own compare, exactly as before Phase 3.6.
  private def slotHits(thread: UInt): UInt =
    VecInit((0 until threadSlotCount).map(s => io.slot_thread(s) === thread)).asUInt

  private def slotFromHits(hits: UInt): UInt =
    if (threadSlotCount == 1) 0.U(threadSlotIdLength.W) else PriorityEncoder(hits)

  private def residentPc(thread: UInt): UInt = {
    val hits = slotHits(thread)
    val slot = slotFromHits(hits)
    Mux(hits.orR, slot_pc(slot), 0.U)
  }

  private def writeSlotPc(thread: UInt, value: UInt): Unit = {
    val hits = slotHits(thread)
    for (s <- 0 until threadSlotCount) {
      when (hits(s)) {
        slot_pc(s) := value
      }
    }
  }

  private def writeSlotPcBySlot(slot: UInt, value: UInt): Unit = {
    for (s <- 0 until threadSlotCount) {
      when (slot === s.U(threadSlotIdLength.W)) {
        slot_pc(s) := value
      }
    }
  }

  private def writeSlotValid(thread: UInt, value: Bool): Unit = {
    val hits = slotHits(thread)
    for (s <- 0 until threadSlotCount) {
      when (hits(s)) {
        slot_meta(s).valid := value
      }
    }
  }

  private def writeSlotRunable(thread: UInt, value: Bool): Unit = {
    val hits = slotHits(thread)
    for (s <- 0 until threadSlotCount) {
      when (hits(s)) {
        slot_meta(s).runable := value
      }
    }
  }

  private def writeSlotValidRunable(thread: UInt, validValue: Bool, runableValue: Bool): Unit = {
    val hits = slotHits(thread)
    for (s <- 0 until threadSlotCount) {
      when (hits(s)) {
        slot_meta(s).valid := validValue
        slot_meta(s).runable := runableValue
      }
    }
  }

  private def writeSlotPriority(thread: UInt, value: UInt): Unit = {
    val hits = slotHits(thread)
    for (s <- 0 until threadSlotCount) {
      when (hits(s)) {
        slot_meta(s).priority := value
      }
    }
  }

  private def writeSlotDeadline(thread: UInt, value: UInt): Unit = {
    val hits = slotHits(thread)
    for (s <- 0 until threadSlotCount) {
      when (hits(s)) {
        slot_meta(s).deadline := value
      }
    }
  }

  private def writeSlotSlice(thread: UInt, value: UInt): Unit = {
    val hits = slotHits(thread)
    for (s <- 0 until threadSlotCount) {
      when (hits(s)) {
        slot_meta(s).slice := value
      }
    }
  }

  private def writeSlotMeta(slot: UInt, meta: ThreadSchedMeta): Unit = {
    for (s <- 0 until threadSlotCount) {
      when (slot === s.U(threadSlotIdLength.W)) {
        slot_meta(s) := meta
      }
    }
  }

  val tpt_ready_bits = tpt_valid_bits & tpt_runable_bits
  for (chunk <- 0 until (threadSupport + 63) / 64) {
    io.tpt_ready_chunks(chunk) :=
      tpt_ready_bits((chunk + 1) * 64 - 1, chunk * 64)
  }
  io.current_thread_out := current_thread

  private val allocChunkBits = 64
  private val allocChunkCount = threadSupport / allocChunkBits
  require(threadSupport % allocChunkBits == 0,
    "ThreadManager allocator requires complete 64-thread chunks")
  val free_thread_chunks = VecInit((0 until allocChunkCount).map { c =>
    ~tpt_valid_bits((c + 1) * allocChunkBits - 1, c * allocChunkBits)
  })
  val free_chunk_summary = VecInit(free_thread_chunks.map(_.orR)).asUInt
  val free_chunk =
    if (allocChunkCount == 1) 0.U else PriorityEncoder(free_chunk_summary)
  val free_offset = PriorityEncoder(free_thread_chunks(free_chunk))
  val create_thread_id =
    (free_chunk * allocChunkBits.U + free_offset)(threadIdLength - 1, 0)
  val no_free_thread = !free_chunk_summary.orR

  val ctrlRankQ = Module(new Queue(new ThreadMetadataRead, 4))
  val dcacheRankQ = Module(new Queue(new ThreadMetadataRead, 4))
  val monitorRankQ = Module(new Queue(new ThreadMetadataRead, 4))

  val tptPriorityCtrlWriteValid = WireDefault(false.B)
  val tptPriorityCtrlWriteThread = WireDefault(0.U(threadIdLength.W))
  val tptPriorityCtrlWriteData = WireDefault(0.U(priorBits.W))
  val tptPriorityAsyncWriteValid = WireDefault(false.B)
  val tptPriorityAsyncWriteThread = WireDefault(0.U(threadIdLength.W))
  val tptPriorityAsyncWriteData = WireDefault(0.U(priorBits.W))
  val tptPriorityPendingValid = RegInit(false.B)
  val tptPriorityPendingThread = RegInit(0.U(threadIdLength.W))
  val tptPriorityPendingData = RegInit(0.U(priorBits.W))
  val tptPriorityWriteValid = WireDefault(false.B)
  val tptPriorityWriteThread = WireDefault(0.U(threadIdLength.W))
  val tptPriorityWriteData = WireDefault(0.U(priorBits.W))
  val tptSliceWriteValid = WireDefault(false.B)
  val tptSliceWriteThread = WireDefault(0.U(threadIdLength.W))
  val tptSliceWriteData = WireDefault(1.U(sliceCntBits.W))
  val tptDeadlineCtrlWriteValid = WireDefault(false.B)
  val tptDeadlineCtrlWriteThread = WireDefault(0.U(threadIdLength.W))
  val tptDeadlineCtrlWriteData = WireDefault(0.U(deadlineBits.W))
  val tptDeadlineAsyncWriteValid = WireDefault(false.B)
  val tptDeadlineAsyncWriteThread = WireDefault(0.U(threadIdLength.W))
  val tptDeadlineAsyncWriteData = WireDefault(0.U(deadlineBits.W))
  val tptDeadlinePendingValid = RegInit(false.B)
  val tptDeadlinePendingThread = RegInit(0.U(threadIdLength.W))
  val tptDeadlinePendingData = RegInit(0.U(deadlineBits.W))
  val tptDeadlineWriteValid = WireDefault(false.B)
  val tptDeadlineWriteThread = WireDefault(0.U(threadIdLength.W))
  val tptDeadlineWriteData = WireDefault(0.U(deadlineBits.W))
  val tptInitIndex = RegInit(0.U(1.W))

  // A later control write to the same entry supersedes an older deferred trap write.
  when (tptPriorityCtrlWriteValid) {
    tptPriorityWriteValid := true.B
    tptPriorityWriteThread := tptPriorityCtrlWriteThread
    tptPriorityWriteData := tptPriorityCtrlWriteData
    val pendingSuperseded = tptPriorityPendingValid &&
      (tptPriorityPendingThread === tptPriorityCtrlWriteThread)
    when (tptPriorityAsyncWriteValid) {
      val canQueueAsync = !tptPriorityPendingValid || pendingSuperseded ||
        (tptPriorityPendingThread === tptPriorityAsyncWriteThread)
      if (coreParams.threadletAreaDebugAssert) {
        assert(canQueueAsync,
          "priority TPT deferred write overflow")
      }
      when (canQueueAsync) {
        tptPriorityPendingValid := true.B
        tptPriorityPendingThread := tptPriorityAsyncWriteThread
        tptPriorityPendingData := tptPriorityAsyncWriteData
      }
    }.elsewhen (pendingSuperseded) {
      tptPriorityPendingValid := false.B
    }
  }.elsewhen (tptPriorityPendingValid) {
    tptPriorityWriteValid := true.B
    tptPriorityWriteThread := tptPriorityPendingThread
    tptPriorityWriteData := tptPriorityPendingData
    when (tptPriorityAsyncWriteValid) {
      tptPriorityPendingThread := tptPriorityAsyncWriteThread
      tptPriorityPendingData := tptPriorityAsyncWriteData
    }.otherwise {
      tptPriorityPendingValid := false.B
    }
  }.elsewhen (tptPriorityAsyncWriteValid) {
    tptPriorityWriteValid := true.B
    tptPriorityWriteThread := tptPriorityAsyncWriteThread
    tptPriorityWriteData := tptPriorityAsyncWriteData
  }

  when (tptDeadlineCtrlWriteValid) {
    tptDeadlineWriteValid := true.B
    tptDeadlineWriteThread := tptDeadlineCtrlWriteThread
    tptDeadlineWriteData := tptDeadlineCtrlWriteData
    val pendingSuperseded = tptDeadlinePendingValid &&
      (tptDeadlinePendingThread === tptDeadlineCtrlWriteThread)
    when (tptDeadlineAsyncWriteValid) {
      val canQueueAsync = !tptDeadlinePendingValid || pendingSuperseded ||
        (tptDeadlinePendingThread === tptDeadlineAsyncWriteThread)
      if (coreParams.threadletAreaDebugAssert) {
        assert(canQueueAsync,
          "deadline TPT deferred write overflow")
      }
      when (canQueueAsync) {
        tptDeadlinePendingValid := true.B
        tptDeadlinePendingThread := tptDeadlineAsyncWriteThread
        tptDeadlinePendingData := tptDeadlineAsyncWriteData
      }
    }.elsewhen (pendingSuperseded) {
      tptDeadlinePendingValid := false.B
    }
  }.elsewhen (tptDeadlinePendingValid) {
    tptDeadlineWriteValid := true.B
    tptDeadlineWriteThread := tptDeadlinePendingThread
    tptDeadlineWriteData := tptDeadlinePendingData
    when (tptDeadlineAsyncWriteValid) {
      tptDeadlinePendingThread := tptDeadlineAsyncWriteThread
      tptDeadlinePendingData := tptDeadlineAsyncWriteData
    }.otherwise {
      tptDeadlinePendingValid := false.B
    }
  }.elsewhen (tptDeadlineAsyncWriteValid) {
    tptDeadlineWriteValid := true.B
    tptDeadlineWriteThread := tptDeadlineAsyncWriteThread
    tptDeadlineWriteData := tptDeadlineAsyncWriteData
  }

  when (!has_init) {
    val initThread = Mux(tptInitIndex === 0.U, 0.U, interrupt_thread.U)
    tptPriorityCtrlWriteValid := true.B
    tptPriorityCtrlWriteThread := initThread
    tptPriorityCtrlWriteData := 0.U
    tptSliceWriteValid := true.B
    tptSliceWriteThread := initThread
    tptSliceWriteData := 1.U
    tptDeadlineCtrlWriteValid := true.B
    tptDeadlineCtrlWriteThread := initThread
    tptDeadlineCtrlWriteData := 0.U
    for (s <- 0 until threadSlotCount) {
      slot_meta(s).slice := 1.U
      slot_meta(s).valid := (s == 0 || s == interrupt_thread).B
      slot_meta(s).runable := (s == 0).B
      slot_meta(s).priority := 0.U
      slot_meta(s).deadline := 0.U
    }
    when (tptInitIndex === 1.U) {
      has_init := true.B
    }.otherwise {
      tptInitIndex := 1.U
    }
  }

  // Asynchronous wakeup from DCache probe matching.
  when (io.dcache_wakeup.valid) {
    writeSlotRunable(io.dcache_wakeup.bits, true.B)
  }

  // Probe and self-eviction events are scanned in the background.
  val probe_fire = io.dcache_probe.valid
  val probe_line = io.dcache_probe.bits(paddrBits - 1, blockOffBits)
  val self_evict_fire = io.dcache_self_evict.valid
  val self_evict_line = io.dcache_self_evict.bits(paddrBits - 1, blockOffBits)
  val monitor_incoming = probe_fire || self_evict_fire
  val monitor_incoming_line = Mux(probe_fire, probe_line, self_evict_line)

  // A queued pre-arm match may cause a conservative wakeup; software rechecks the condition.
  private val monitorEventQ = Module(new Queue(new MonitorEvent, 4))
  monitorEventQ.io.enq.valid := monitor_incoming
  monitorEventQ.io.enq.bits.line := monitor_incoming_line

  val monitor_dual_event_conflict =
    probe_fire && self_evict_fire && (probe_line =/= self_evict_line)
  val monitor_event_overflow =
    monitor_incoming && (!monitorEventQ.io.enq.ready || monitor_dual_event_conflict)
  val monitor_overflow_pending = RegInit(false.B)

  val monitor_scan_active = RegInit(false.B)
  val monitor_scan_line = RegInit(0.U(monitorLineBits.W))
  val monitor_scan_overflow = RegInit(false.B)
  val monitor_scan_row = RegInit(0.U(monitorRowBits.W))
  val monitor_hit_pending = RegInit(0.U(monitorScanBanks.W))
  val monitor_hit_row = RegInit(0.U(monitorRowBits.W))
  val monitor_hit_last = RegInit(false.B)

  val monitor_walker_idle = !monitor_scan_active && !monitor_hit_pending.orR
  val monitor_start_overflow = monitor_walker_idle && monitor_overflow_pending
  monitorEventQ.io.deq.ready := monitor_walker_idle && !monitor_overflow_pending
  when (monitor_start_overflow) {
    monitor_scan_active := true.B
    monitor_scan_line := 0.U
    monitor_scan_overflow := true.B
    monitor_scan_row := 0.U
    monitor_overflow_pending := false.B
  }.elsewhen (monitorEventQ.io.deq.fire) {
    monitor_scan_active := true.B
    monitor_scan_line := monitorEventQ.io.deq.bits.line
    monitor_scan_overflow := false.B
    monitor_scan_row := 0.U
  }
  when (monitor_event_overflow) {
    monitor_overflow_pending := true.B
  }

  val monitor_scan_tids = Wire(Vec(monitorScanBanks, UInt(threadIdLength.W)))
  val monitor_scan_indices = Wire(Vec(monitorScanBanks, UInt(monitorIdBits.W)))
  val monitor_scan_hits = Wire(Vec(monitorScanBanks, Bool()))
  for (bank <- 0 until monitorScanBanks) {
    val localTid =
      (monitor_scan_row * monitorScanBanks.U + bank.U)(monitorIdBits - 1, 0)
    val tid = localTid.pad(threadIdLength)
    monitor_scan_indices(bank) := localTid
    monitor_scan_tids(bank) := tid
    monitor_scan_hits(bank) :=
      monitor_scan_active && !monitor_hit_pending.orR &&
        tptValid(tid) && mon_valid(localTid) &&
        (monitor_scan_overflow ||
          (monitorLineBanks(bank)(monitor_scan_row) === monitor_scan_line))
  }
  val monitor_scan_hit_mask = monitor_scan_hits.asUInt
  val monitor_scan_is_last = monitor_scan_row === (monitorScanRows - 1).U

  when (monitor_scan_active && !monitor_hit_pending.orR) {
    when (monitor_scan_hit_mask.orR) {
      monitor_hit_pending := monitor_scan_hit_mask
      monitor_hit_row := monitor_scan_row
      monitor_hit_last := monitor_scan_is_last
      for (bank <- 0 until monitorScanBanks) {
        when (monitor_scan_hits(bank)) {
          mon_valid(monitor_scan_indices(bank)) := false.B
        }
      }
    }.elsewhen (monitor_scan_is_last) {
      monitor_scan_active := false.B
    }.otherwise {
      monitor_scan_row := monitor_scan_row + 1.U
    }
  }

  val monitor_hit_bank = PriorityEncoder(monitor_hit_pending)
  val monitor_hit_index =
    (monitor_hit_row * monitorScanBanks.U + monitor_hit_bank)(monitorIdBits - 1, 0)
  val monitor_hit_thread = monitor_hit_index.pad(threadIdLength)
  val monitor_hit_available = monitor_hit_pending.orR
  val monitor_hit_live = monitor_hit_available && tptValid(monitor_hit_thread)
  val monitor_hit_needs_rank = monitor_hit_live && !tptRunable(monitor_hit_thread)
  val monitor_hit_fire =
    monitor_hit_available && (!monitor_hit_needs_rank || monitorRankQ.io.enq.ready)
  val monitor_wakeup_event_valid = monitor_hit_fire && monitor_hit_needs_rank
  val monitor_hit_remaining =
    monitor_hit_pending & ~UIntToOH(monitor_hit_bank, monitorScanBanks)
  when (monitor_hit_fire) {
    monitor_hit_pending := monitor_hit_remaining
    when (monitor_hit_live) {
      when (tptRunable(monitor_hit_thread)) {
        mon_pending(monitor_hit_index) := true.B
      }.otherwise {
        writeSlotRunable(monitor_hit_thread, true.B)
        mon_pending(monitor_hit_index) := false.B
      }
    }
    when (!monitor_hit_remaining.orR) {
      when (monitor_hit_last) {
        monitor_scan_active := false.B
      }.otherwise {
        monitor_scan_row := monitor_hit_row + 1.U
      }
    }
  }

  private def monitorMatchPending(thread: UInt): Bool = {
    val supported = monitorSupported(thread)
    val idx = monitorIndex(thread)
    val bank = idx(monitorBankBits - 1, 0)
    val row = idx(monitorIdBits - 1, monitorBankBits)
    val registered = monitor_hit_pending(bank) && (monitor_hit_row === row)
    val current = monitor_scan_active && !monitor_hit_pending.orR &&
      monitor_scan_hits(bank) && (monitor_scan_row === row)
    supported && (registered || current)
  }

  val ctrl_thread_yield_has_pending = monitorPending(io.ctrl.bits.thread) ||
    monitorMatchPending(io.ctrl.bits.thread)
  val coldPcCreateWriteValid = WireDefault(false.B)
  val coldPcCreateWriteThread = WireDefault(0.U(threadIdLength.W))
  val coldPcCreateWriteData = WireDefault(0.U(vaddrBitsExtended.W))

  io.ctrl.ready := true.B
  when (io.ctrl.fire) {
    when (io.ctrl.bits.init) {
      ctx_switch_enable := true.B
    }

    when (io.ctrl.bits.create) {
      when (!no_free_thread) {
        coldPcCreateWriteValid := true.B
        coldPcCreateWriteThread := create_thread_id
        coldPcCreateWriteData := io.ctrl.bits.create_pc
        writeSlotPc(create_thread_id, io.ctrl.bits.create_pc)
        tptPriorityCtrlWriteValid := true.B
        tptPriorityCtrlWriteThread := create_thread_id
        tptPriorityCtrlWriteData := 0.U
        tptSliceWriteValid := true.B
        tptSliceWriteThread := create_thread_id
        tptSliceWriteData := 1.U
        tptDeadlineCtrlWriteValid := true.B
        tptDeadlineCtrlWriteThread := create_thread_id
        tptDeadlineCtrlWriteData := 0.U
        writeSlotValidRunable(create_thread_id, true.B, false.B)
      }
    }

    when (io.ctrl.bits.halt) {
      writeSlotValidRunable(io.ctrl.bits.thread, false.B, false.B)
    }

    when (io.ctrl.bits.yields) {
      val t = io.ctrl.bits.thread
      val has_pending = monitorPending(t) || monitorMatchPending(t)
      when (has_pending) {
        when (monitorSupported(t)) {
          val idx = monitorIndex(t)
          mon_valid(idx) := false.B
          mon_pending(idx) := false.B
        }
      }.otherwise {
        writeSlotRunable(t, false.B)
      }
    }

    when (io.ctrl.bits.set_prior) {
      tptPriorityCtrlWriteValid := true.B
      tptPriorityCtrlWriteThread := io.ctrl.bits.prior_thread
      tptPriorityCtrlWriteData := io.ctrl.bits.prior
      writeSlotPriority(io.ctrl.bits.prior_thread, io.ctrl.bits.prior)
    }

    when (io.ctrl.bits.set_slice) {
      val t = io.ctrl.bits.slice_thread
      val cycles = io.ctrl.bits.slice
      val ticks_raw = (cycles + ((1 << sliceTickShift) - 1).U) >> sliceTickShift
      val ticks_nz = Mux(ticks_raw === 0.U, 1.U, ticks_raw)
      val ticks_clamped =
        Mux(ticks_nz > sliceMaxTicks.U, sliceMaxTicks.U, ticks_nz)(sliceCntBits - 1, 0)
      tptSliceWriteValid := true.B
      tptSliceWriteThread := t
      tptSliceWriteData := ticks_clamped
      writeSlotSlice(t, ticks_clamped)
    }

    // PASS is a manual timeslice boundary.
    when (io.ctrl.bits.pass) {
      tptDeadlineCtrlWriteValid := true.B
      tptDeadlineCtrlWriteThread := io.ctrl.bits.thread
      tptDeadlineCtrlWriteData := 0.U
      writeSlotDeadline(io.ctrl.bits.thread, 0.U)
    }

    when (io.ctrl.bits.set_deadline) {
      val t = io.ctrl.bits.deadline_thread
      val cycles = io.ctrl.bits.deadline

      when (cycles === 0.U) {
        tptDeadlineCtrlWriteValid := true.B
        tptDeadlineCtrlWriteThread := t
        tptDeadlineCtrlWriteData := 0.U
        writeSlotDeadline(t, 0.U)
      }.otherwise {
        val ticks_raw = (cycles + ((1 << sliceTickShift) - 1).U) >> sliceTickShift
        val ticks_nz = Mux(ticks_raw === 0.U, 1.U, ticks_raw)
        val deadline_max = ((BigInt(1) << deadlineBits) - 1).U(deadlineBits.W)
        val deadline_next = Mux(ticks_nz > deadline_max, deadline_max, ticks_nz)(deadlineBits - 1, 0)
        tptDeadlineCtrlWriteValid := true.B
        tptDeadlineCtrlWriteThread := t
        tptDeadlineCtrlWriteData := deadline_next
        writeSlotDeadline(t, deadline_next)
      }
    }

    when (io.ctrl.bits.wakeup) {
      writeSlotRunable(io.ctrl.bits.wakeup_thread, true.B)
    }

    when (io.ctrl.bits.dcache_monitor_set) {
      val t = io.ctrl.bits.thread
      when (monitorSupported(t)) {
        val idx = monitorIndex(t)
        val bank = idx(monitorBankBits - 1, 0)
        val row = idx(monitorIdBits - 1, monitorBankBits)
        for (b <- 0 until monitorScanBanks) {
          when (bank === b.U) {
            monitorLineBanks(b)(row) :=
              io.ctrl.bits.dcache_monitor_addr(paddrBits - 1, blockOffBits)
          }
        }
        mon_valid(idx) := true.B
        mon_pending(idx) := false.B
      }
    }

    when (io.ctrl.bits.dcache_monitor_clear) {
      val t = io.ctrl.bits.thread
      when (monitorSupported(t)) {
        val idx = monitorIndex(t)
        mon_valid(idx) := false.B
        mon_pending(idx) := false.B
      }
    }

    if (coreParams.threadletAreaDebugPrintf) {
      when (io.ctrl.bits.syn_print) {
        midas.targetutils.SynthesizePrintf(printf("[State #9.3][cpu=%d] stage: %d, addr: 0x%x (%d)\n", io.hartid, io.ctrl.bits.syn_stage, io.ctrl.bits.syn_data, io.ctrl.bits.syn_data))
      }
    }

    when (io.ctrl.bits.set_base) {
      interrupt_base := io.ctrl.bits.interrupt_base
    }

    when (io.ctrl.bits.eret) {
      eret := true.B
    }
  }
  io.ret.valid := io.ctrl.fire && io.ctrl.bits.create && !no_free_thread
  io.ret.bits.pc := io.ctrl.bits.create_pc
  io.ret.bits.thread := create_thread_id
  io.ret.bits.prior := DontCare
  io.ret.bits.status := DontCare

  val wire_valid = WireDefault(false.B)
  val wire_thread = WireDefault(0.U(threadIdLength.W))
  val wire_pc = WireDefault(0.U(vaddrBitsExtended.W))

  private def issuePc(thread: UInt): UInt =
    Mux(wire_valid && thread === wire_thread, wire_pc, residentPc(thread))

  val should_switch_yield_halt = io.ctrl.fire &&
    (io.ctrl.bits.yields || io.ctrl.bits.halt) &&
    io.ctrl.bits.thread === current_thread
  val should_switch_pass = io.ctrl.fire &&
    io.ctrl.bits.pass &&
    io.ctrl.bits.thread === current_thread
  val should_switch = should_switch_yield_halt || should_switch_pass

  val set_deadline_fire = io.ctrl.fire && io.ctrl.bits.set_deadline
  val set_deadline_tid = io.ctrl.bits.deadline_thread
  val tick_count_fire = sched_tick_fire &&
    (current_thread =/= interrupt_thread.U) &&
    !io.interrupt &&
    !should_switch
  val tick_accounting_fire = tick_count_fire &&
    !(io.xcpt.valid && current_thread === io.xcpt.bits.thread)
  val cur_slice_finishes_now = tick_accounting_fire && slice_will_expire
  val clear_current_deadline_now = (cur_slice_finishes_now || should_switch_pass) &&
    !(set_deadline_fire && (set_deadline_tid === current_thread))
  val cur_yield_has_pending = monitorPending(current_thread) ||
    monitorMatchPending(current_thread)
  val drop_current_for_yield = io.ctrl.fire &&
    io.ctrl.bits.yields &&
    (io.ctrl.bits.thread === current_thread) &&
    !cur_yield_has_pending
  val drop_current_for_halt = io.ctrl.fire &&
    io.ctrl.bits.halt &&
    (io.ctrl.bits.thread === current_thread)
  val do_intr_eret_switch = io.xcpt.valid &&
    (current_thread === io.xcpt.bits.thread) &&
    (current_thread === interrupt_thread.U) &&
    io.xcpt.bits.eret &&
    eret
  val drop_current_for_intr_eret = do_intr_eret_switch

  private def threadOH(thread: UInt): UInt = UIntToOH(thread, threadSupport)
  private def gatedThreadOH(enable: Bool, thread: UInt): UInt =
    Mux(enable, threadOH(thread), 0.U(threadSupport.W))

  val create_metadata_fire = io.ctrl.fire && io.ctrl.bits.create && !no_free_thread
  val halt_metadata_fire = io.ctrl.fire && io.ctrl.bits.halt
  val yield_sleep_fire = io.ctrl.fire && io.ctrl.bits.yields &&
    !ctrl_thread_yield_has_pending
  val wakeup_metadata_fire = io.ctrl.fire && io.ctrl.bits.wakeup
  val intr_eret_sleep_fire = do_intr_eret_switch && !io.interrupt
  if (coreParams.threadletAreaDebugAssert) {
    assert(PopCount(Seq(
      create_metadata_fire,
      halt_metadata_fire,
      yield_sleep_fire,
      wakeup_metadata_fire)) <= 1.U,
      "threadlet bitmap ctrl writers must be mutually exclusive")
  }

  // Writer-merge: the ctrl-sourced bitmap writers (create/halt/yield/wakeup) are
  // mutually exclusive one-hot decodes (ThreadInstDecode), so they collapse to a
  // single dynamic decode per bitmap; interrupt/eret target the constant
  // interrupt thread. The layered structure below reproduces the original
  // per-source chain precedence (a later source overrides an earlier one on a
  // shared bit): sets from dcache/monitor first, then the single ctrl write,
  // then interrupt set, then eret clear. This keeps every update in the same
  // cycle as before (no queue, no added latency) while removing the 8/3-level
  // 512-bit one-hot chains.

  // valid: create -> set(create_id); halt -> clear(ctrl.thread); interrupt -> set(1).
  val valid_ctrl_thread = Mux(create_metadata_fire, create_thread_id, io.ctrl.bits.thread)
  val valid_ctrl_oh = gatedThreadOH(create_metadata_fire || halt_metadata_fire, valid_ctrl_thread)
  val tpt_valid_after_ctrl =
    Mux(create_metadata_fire, tpt_valid_bits | valid_ctrl_oh, tpt_valid_bits & ~valid_ctrl_oh)
  val tpt_valid_bits_next =
    tpt_valid_after_ctrl | gatedThreadOH(io.interrupt, interrupt_thread.U)

  // runnable: dcache/monitor set (L1) < single ctrl write (L2) < interrupt set (L3)
  // < eret clear (L4). interrupt (L3) and eret (L4) are mutually exclusive.
  val run_ctrl_thread = Mux(create_metadata_fire, create_thread_id,
    Mux(wakeup_metadata_fire, io.ctrl.bits.wakeup_thread, io.ctrl.bits.thread))
  val run_ctrl_en = create_metadata_fire || halt_metadata_fire ||
    yield_sleep_fire || wakeup_metadata_fire
  val run_ctrl_oh = gatedThreadOH(run_ctrl_en, run_ctrl_thread)
  val tpt_runable_l1 = tpt_runable_bits |
    gatedThreadOH(io.dcache_wakeup.valid, io.dcache_wakeup.bits) |
    gatedThreadOH(monitor_wakeup_event_valid, monitor_hit_thread)
  val tpt_runable_l2 =
    Mux(wakeup_metadata_fire, tpt_runable_l1 | run_ctrl_oh, tpt_runable_l1 & ~run_ctrl_oh)
  val tpt_runable_l3 = tpt_runable_l2 | gatedThreadOH(io.interrupt, interrupt_thread.U)
  val tpt_runable_bits_next =
    tpt_runable_l3 & ~gatedThreadOH(intr_eret_sleep_fire, interrupt_thread.U)

  tpt_valid_bits := tpt_valid_bits_next
  tpt_runable_bits := tpt_runable_bits_next

  private def runableForPick(baseRunable: Bool, t: UInt): Bool = {
    val is_current = t === current_thread
    val drop_current = is_current &&
      (drop_current_for_yield || drop_current_for_halt || drop_current_for_intr_eret)
    baseRunable && !drop_current
  }

  private def metaForPick(base: ThreadSchedMeta, t: UInt): ThreadSchedMeta = {
    val meta = WireDefault(base)
    meta.runable := runableForPick(base.runable, t)
    meta.deadline := Mux((t === current_thread) && clear_current_deadline_now, 0.U, base.deadline)
    meta
  }

  when (sched_tick_fire && clear_current_deadline_now) {
    tptDeadlineCtrlWriteValid := true.B
    tptDeadlineCtrlWriteThread := current_thread
    tptDeadlineCtrlWriteData := 0.U
    writeSlotDeadline(current_thread, 0.U)
  }

  io.slot_load_commit.valid := false.B
  io.slot_load_commit.bits := 0.U.asTypeOf(new ThreadSlotLoadCommit)

  // Phase 3.5: the scan and rank/load TPT reads share one physical read port,
  // one forwarding network, and one metadata-compute path. The unified service
  // is defined after the rank-change arbiter below (it needs rankChangeArb).

  val ctrl_rank_event_valid = WireDefault(false.B)
  val ctrl_rank_event_thread = WireDefault(0.U(threadIdLength.W))
  val ctrl_rank_event_rerank = WireDefault(true.B)
  when (io.ctrl.fire && io.ctrl.bits.set_prior) {
    ctrl_rank_event_valid := true.B
    ctrl_rank_event_thread := io.ctrl.bits.prior_thread
  }.elsewhen (io.ctrl.fire && io.ctrl.bits.set_slice) {
    ctrl_rank_event_valid := true.B
    ctrl_rank_event_thread := io.ctrl.bits.slice_thread
    ctrl_rank_event_rerank := false.B
  }.elsewhen (io.ctrl.fire && io.ctrl.bits.set_deadline) {
    ctrl_rank_event_valid := true.B
    ctrl_rank_event_thread := io.ctrl.bits.deadline_thread
  }.elsewhen (io.ctrl.fire && io.ctrl.bits.wakeup) {
    ctrl_rank_event_valid := true.B
    ctrl_rank_event_thread := io.ctrl.bits.wakeup_thread
  }.elsewhen (io.ctrl.fire && io.ctrl.bits.halt) {
    ctrl_rank_event_valid := true.B
    ctrl_rank_event_thread := io.ctrl.bits.thread
  }.elsewhen (io.ctrl.fire && io.ctrl.bits.yields && !ctrl_thread_yield_has_pending) {
    ctrl_rank_event_valid := true.B
    ctrl_rank_event_thread := io.ctrl.bits.thread
  }.elsewhen (io.ctrl.fire && io.ctrl.bits.pass) {
    ctrl_rank_event_valid := true.B
    ctrl_rank_event_thread := io.ctrl.bits.thread
  }.elsewhen (sched_tick_fire && clear_current_deadline_now) {
    ctrl_rank_event_valid := true.B
    ctrl_rank_event_thread := current_thread
  }.elsewhen (io.ctrl.fire && io.ctrl.bits.create && !no_free_thread) {
    ctrl_rank_event_valid := true.B
    ctrl_rank_event_thread := create_thread_id
  }

  ctrlRankQ.io.enq.valid := ctrl_rank_event_valid
  ctrlRankQ.io.enq.bits.thread := ctrl_rank_event_thread
  ctrlRankQ.io.enq.bits.rerank := ctrl_rank_event_rerank
  if (coreParams.threadletAreaDebugAssert) {
    assert(!ctrl_rank_event_valid || ctrlRankQ.io.enq.ready,
      "threadlet control rank-change queue overflow")
  }

  dcacheRankQ.io.enq.valid := io.dcache_wakeup.valid
  dcacheRankQ.io.enq.bits.thread := io.dcache_wakeup.bits
  dcacheRankQ.io.enq.bits.rerank := true.B
  if (coreParams.threadletAreaDebugAssert) {
    assert(!io.dcache_wakeup.valid || dcacheRankQ.io.enq.ready,
      "threadlet DCache wakeup rank-change queue overflow")
  }

  monitorRankQ.io.enq.valid := monitor_hit_needs_rank
  monitorRankQ.io.enq.bits.thread := monitor_hit_thread
  monitorRankQ.io.enq.bits.rerank := true.B

  val rankChangeArb = Module(new RRArbiter(new ThreadMetadataRead, 3))
  rankChangeArb.io.in(0) <> ctrlRankQ.io.deq
  rankChangeArb.io.in(1) <> dcacheRankQ.io.deq
  rankChangeArb.io.in(2) <> monitorRankQ.io.deq

  val rankSnapshotQ = Module(new Queue(new ThreadRankChange, 2, pipe = true, flow = false))
  val tpt_rank_outstanding = RegInit(0.U(2.W))

  // ---- Unified TPT metadata read service (Phase 3.5) ----
  // One physical read port on each TPT SyncReadMem, one forwarding-register set,
  // and one metadata-compute path serve all three requesters. Priority:
  //   load (replacement completion, most timing-critical) > rank-change > scan.
  // scan (TCM background read) yields the shared port and may wait extra cycles;
  // the TCM tolerates variable scan latency, and load/rank keep their prior
  // timing (load always wins; rank keeps the same 2-outstanding reservation).
  val kScan = 0.U(2.W)
  val kRank = 1.U(2.W)
  val kLoad = 2.U(2.W)
  val tpt_load_issue = io.slot_load_request.valid
  rankChangeArb.io.out.ready := !tpt_load_issue && (tpt_rank_outstanding < 2.U)
  val tpt_rank_issue = rankChangeArb.io.out.fire
  io.tpt_read_req.ready := !tpt_load_issue && !tpt_rank_issue
  val tpt_scan_issue = io.tpt_read_req.fire
  val tpt_rd_fire = tpt_load_issue || tpt_rank_issue || tpt_scan_issue
  val tpt_rd_kind = Mux(tpt_load_issue, kLoad, Mux(tpt_rank_issue, kRank, kScan))
  val tpt_rd_thread = Mux(tpt_load_issue, io.slot_load_request.bits.thread,
    Mux(tpt_rank_issue, rankChangeArb.io.out.bits.thread, io.tpt_read_req.bits.thread))

  // Cold PC read (load path only; separate banked mems, keyed on the load).
  val coldPcLoadBank = io.slot_load_request.bits.thread(pcBankBits - 1, 0)
  val coldPcLoadRow = io.slot_load_request.bits.thread(threadIdLength - 1, pcBankBits)
  val coldPcLoadDataByBank = Wire(Vec(pcBankCount, UInt(vaddrBitsExtended.W)))
  for (bank <- 0 until pcBankCount) {
    coldPcLoadDataByBank(bank) :=
      coldPcBanks(bank).read(coldPcLoadRow, tpt_load_issue && coldPcLoadBank === bank.U)
  }
  val coldPcLoadRespBank = RegEnable(coldPcLoadBank, tpt_load_issue)
  val coldPcLoadRespData = coldPcLoadDataByBank(coldPcLoadRespBank)

  // Single physical read port per TPT SyncReadMem.
  val tpt_rd_priority_data = tptPriorityMem.read(tpt_rd_thread, tpt_rd_fire)
  val tpt_rd_slice_data = tptSliceMem.read(tpt_rd_thread, tpt_rd_fire)
  val tpt_rd_deadline_data = tptDeadlineMem.read(tpt_rd_thread, tpt_rd_fire)

  // Single response context + single forwarding-register set. Data/flag regs are
  // gated by tpt_rd_resp_valid, so they need no reset value.
  val tpt_rd_resp_valid = RegNext(tpt_rd_fire, false.B)
  val tpt_rd_resp_kind = RegEnable(tpt_rd_kind, tpt_rd_fire)
  val tpt_rd_resp_thread = RegEnable(tpt_rd_thread, tpt_rd_fire)
  val tpt_rd_resp_slot = RegEnable(io.slot_load_request.bits.slot, tpt_rd_fire)
  val tpt_rd_resp_scan_req = RegEnable(io.tpt_read_req.bits, tpt_rd_fire)
  val tpt_rd_resp_rerank = RegEnable(rankChangeArb.io.out.bits.rerank, tpt_rd_fire)
  val tpt_rd_priority_forward = RegEnable(
    tptPriorityWriteValid && (tptPriorityWriteThread === tpt_rd_thread), tpt_rd_fire)
  val tpt_rd_priority_forward_data = RegEnable(tptPriorityWriteData, tpt_rd_fire)
  val tpt_rd_slice_forward = RegEnable(
    tptSliceWriteValid && (tptSliceWriteThread === tpt_rd_thread), tpt_rd_fire)
  val tpt_rd_slice_forward_data = RegEnable(tptSliceWriteData, tpt_rd_fire)
  val tpt_rd_deadline_forward = RegEnable(
    tptDeadlineWriteValid && (tptDeadlineWriteThread === tpt_rd_thread), tpt_rd_fire)
  val tpt_rd_deadline_forward_data = RegEnable(tptDeadlineWriteData, tpt_rd_fire)
  // Phase 3.7: read valid/runnable once at the issue cycle (single 512:1 on the
  // next-state bitmap, registered) instead of a response-cycle 512:1 that fans
  // out to all three consumers and gets duplicated. Registered value equals
  // tpt_*_bits(resp cycle)[resp_thread]; response-cycle events are live-forwarded
  // below, so the result is bit-identical to reading tpt_*_bits_next at response.
  val tpt_rd_valid_reg = RegEnable(tptBit(tpt_valid_bits_next, tpt_rd_thread), tpt_rd_fire)
  val tpt_rd_runable_reg = RegEnable(tptBit(tpt_runable_bits_next, tpt_rd_thread), tpt_rd_fire)

  // Single metadata compute: registered forward from the issue cycle plus a live
  // write-forward at the response cycle (both preserved exactly as before).
  val tpt_rd_resp_meta = Wire(new ThreadSchedMeta)
  // Live-forward the response-cycle bitmap events onto resp_thread, mirroring the
  // writer-merge precedence exactly (valid: create-set < halt-clear < interrupt-set;
  // runnable: dcache/monitor-set < ctrl < interrupt-set < eret-clear).
  val respThread = tpt_rd_resp_thread
  val vfwd_create = create_metadata_fire && (create_thread_id === respThread)
  val vfwd_halt = halt_metadata_fire && (io.ctrl.bits.thread === respThread)
  val vfwd_intr = io.interrupt && (interrupt_thread.U === respThread)
  tpt_rd_resp_meta.valid :=
    vfwd_intr || Mux(vfwd_create, true.B, Mux(vfwd_halt, false.B, tpt_rd_valid_reg))
  val rfwd_dcache = io.dcache_wakeup.valid && (io.dcache_wakeup.bits === respThread)
  val rfwd_mon = monitor_wakeup_event_valid && (monitor_hit_thread === respThread)
  val rfwd_wakeup = wakeup_metadata_fire && (io.ctrl.bits.wakeup_thread === respThread)
  val rfwd_ctrl_clr = (create_metadata_fire && (create_thread_id === respThread)) ||
    (halt_metadata_fire && (io.ctrl.bits.thread === respThread)) ||
    (yield_sleep_fire && (io.ctrl.bits.thread === respThread))
  val rfwd_intr = io.interrupt && (interrupt_thread.U === respThread)
  val rfwd_eret = intr_eret_sleep_fire && (interrupt_thread.U === respThread)
  val rfwd_l1 = tpt_rd_runable_reg || rfwd_dcache || rfwd_mon
  val rfwd_l2 = Mux(rfwd_wakeup, true.B, Mux(rfwd_ctrl_clr, false.B, rfwd_l1))
  tpt_rd_resp_meta.runable := (rfwd_l2 || rfwd_intr) && !rfwd_eret
  tpt_rd_resp_meta.priority :=
    Mux(tptPriorityWriteValid && (tptPriorityWriteThread === tpt_rd_resp_thread),
      tptPriorityWriteData,
      Mux(tpt_rd_priority_forward, tpt_rd_priority_forward_data, tpt_rd_priority_data))
  tpt_rd_resp_meta.slice :=
    Mux(tptSliceWriteValid && (tptSliceWriteThread === tpt_rd_resp_thread),
      tptSliceWriteData,
      Mux(tpt_rd_slice_forward, tpt_rd_slice_forward_data, tpt_rd_slice_data))
  tpt_rd_resp_meta.deadline :=
    Mux(tptDeadlineWriteValid && (tptDeadlineWriteThread === tpt_rd_resp_thread),
      tptDeadlineWriteData,
      Mux(tpt_rd_deadline_forward, tpt_rd_deadline_forward_data, tpt_rd_deadline_data))

  val tpt_rd_resp_is_scan = tpt_rd_resp_valid && (tpt_rd_resp_kind === kScan)
  val tpt_rd_resp_is_rank = tpt_rd_resp_valid && (tpt_rd_resp_kind === kRank)
  val tpt_rd_resp_is_load = tpt_rd_resp_valid && (tpt_rd_resp_kind === kLoad)

  // Scan response -> TCM scanner (Valid, always sampled).
  io.tpt_read_resp.valid := tpt_rd_resp_is_scan
  io.tpt_read_resp.bits.req := tpt_rd_resp_scan_req
  io.tpt_read_resp.bits.meta := tpt_rd_resp_meta

  // Rank response -> rankSnapshotQ.
  rankSnapshotQ.io.enq.valid := tpt_rd_resp_is_rank
  rankSnapshotQ.io.enq.bits.thread := tpt_rd_resp_thread
  rankSnapshotQ.io.enq.bits.meta := tpt_rd_resp_meta
  rankSnapshotQ.io.enq.bits.rerank := tpt_rd_resp_rerank

  // Load response -> slot metadata/PC commit.
  when (tpt_rd_resp_is_load) {
    writeSlotMeta(tpt_rd_resp_slot, tpt_rd_resp_meta)
    writeSlotPcBySlot(tpt_rd_resp_slot, coldPcLoadRespData)
    io.slot_load_commit.valid := true.B
    io.slot_load_commit.bits.thread := tpt_rd_resp_thread
    io.slot_load_commit.bits.slot := tpt_rd_resp_slot
    io.slot_load_commit.bits.meta := tpt_rd_resp_meta
  }
  io.rank_change.valid := rankSnapshotQ.io.deq.valid
  io.rank_change.bits := rankSnapshotQ.io.deq.bits
  rankSnapshotQ.io.deq.ready := io.rank_change.ready
  when (tpt_rank_issue && !rankSnapshotQ.io.deq.fire) {
    tpt_rank_outstanding := tpt_rank_outstanding + 1.U
  }.elsewhen (!tpt_rank_issue && rankSnapshotQ.io.deq.fire) {
    tpt_rank_outstanding := tpt_rank_outstanding - 1.U
  }
  if (coreParams.threadletAreaDebugAssert) {
    assert(!tpt_rd_resp_is_rank || rankSnapshotQ.io.enq.ready,
      "rank TPT response queue must reserve space before issuing a read")
  }
  io.metadata_pending :=
    ctrl_rank_event_valid || io.dcache_wakeup.valid || monitor_hit_needs_rank ||
      ctrlRankQ.io.deq.valid || dcacheRankQ.io.deq.valid || monitorRankQ.io.deq.valid ||
      rankChangeArb.io.out.valid || (tpt_rank_outstanding =/= 0.U) ||
      tptPriorityAsyncWriteValid || tptPriorityPendingValid ||
      tptDeadlineAsyncWriteValid || tptDeadlinePendingValid

  val slot_tid = io.slot_thread
  val slot_meta_eff = Wire(Vec(threadSlotCount, new ThreadSchedMeta))
  for (s <- 0 until threadSlotCount) {
    slot_meta_eff(s) := metaForPick(slot_meta(s), slot_tid(s))
  }
  io.slot_meta := slot_meta_eff

  val slot_runnable = Wire(Vec(threadSlotCount, Bool()))
  val slot_priority = Wire(Vec(threadSlotCount, UInt(priorBits.W)))
  val slot_deadline = Wire(Vec(threadSlotCount, UInt(deadlineBits.W)))
  for (s <- 0 until threadSlotCount) {
    slot_runnable(s) := slot_meta_eff(s).valid && slot_meta_eff(s).runable && !io.slot_sched_blocked(s)
    slot_priority(s) := slot_meta_eff(s).priority
    slot_deadline(s) := slot_meta_eff(s).deadline
  }

  val runnable_mask = slot_runnable.asUInt
  val top_prio = reduceTreeMax((0 until threadSlotCount).map(i =>
    Mux(runnable_mask(i), slot_priority(i), 0.U(priorBits.W))))
  val top_mask = VecInit((0 until threadSlotCount).map(i =>
    runnable_mask(i) && (slot_priority(i) === top_prio))).asUInt
  val top_deadline_mask = VecInit((0 until threadSlotCount).map(i =>
    top_mask(i) && (slot_deadline(i) =/= 0.U))).asUInt
  val deadline_max = ((BigInt(1) << deadlineBits) - 1).U(deadlineBits.W)
  val sched_key = Wire(Vec(threadSlotCount, UInt(deadlineBits.W)))
  for (s <- 0 until threadSlotCount) {
    sched_key(s) := Mux(slot_deadline(s) =/= 0.U, slot_deadline(s), deadline_max)
  }
  val pick_key_candidates = (0 until threadSlotCount).map(i =>
    Mux(top_deadline_mask(i), sched_key(i), deadline_max))
  val pick_min_key = reduceTreeMin(pick_key_candidates)
  val pick_deadline_mask = VecInit((0 until threadSlotCount).map(i =>
    top_deadline_mask(i) && (sched_key(i) === pick_min_key))).asUInt
  val pick_mask = Mux(top_deadline_mask.orR, pick_deadline_mask, top_mask)
  val (sched_pick_found, sched_pick_thread) =
    slotPickAfter(current_thread, pick_mask, slot_tid)

  val current_slot_hits = slotHits(current_thread)
  val current_slot = if (threadSlotCount == 1) 0.U(threadSlotIdLength.W) else PriorityEncoder(current_slot_hits)
  val cur_prio = Mux(current_slot_hits.orR, slot_meta_eff(current_slot).priority, 0.U(priorBits.W))
  private def residentSlice(thread: UInt): UInt = {
    val hits = slotHits(thread)
    val slot = if (threadSlotCount == 1) 0.U(threadSlotIdLength.W) else PriorityEncoder(hits)
    Mux(hits.orR, slot_meta_eff(slot).slice, 1.U(sliceCntBits.W))
  }
  val preempt_hi_eligible = VecInit((0 until threadSlotCount).map { s =>
    slot_meta_eff(s).valid && slot_meta_eff(s).runable &&
      !io.slot_sched_blocked(s) && (slot_meta_eff(s).priority > cur_prio)
  }).asUInt
  val (preempt_hi_found, preempt_hi_thread) =
    slotPickAfter(current_thread, preempt_hi_eligible, slot_tid)


  wire_valid := io.wb_info.valid || io.mem_info.valid
  when (io.wb_info.valid) {
    writeSlotPc(io.wb_info.bits.thread, io.wb_info.bits.pc)
    wire_thread := io.wb_info.bits.thread
    wire_pc := io.wb_info.bits.pc
  }. elsewhen(io.mem_info.valid) {
    writeSlotPc(io.mem_info.bits.thread, io.mem_info.bits.pc)
    wire_thread := io.mem_info.bits.thread
    wire_pc := io.mem_info.bits.pc
  }
  io.mem_pc := residentPc(io.mem_req)

  io.new_thread_issue_req.valid := false.B
  io.new_thread_issue_req.bits.pc := DontCare
  io.new_thread_issue_req.bits.thread := DontCare
  io.new_thread_issue_req.bits.prior := DontCare
  io.new_thread_issue_req.bits.status := DontCare
  val dbg_sched_valid = WireDefault(false.B)
  val dbg_sched_cause = WireDefault(0.U(3.W))
  val dbg_sched_to = WireDefault(current_thread)

  when (io.interrupt) {
    // redirect interrupt to a thread
    tptPriorityAsyncWriteValid := true.B
    tptPriorityAsyncWriteThread := interrupt_thread.U
    tptPriorityAsyncWriteData := interrupt_prior.U
    tptDeadlineAsyncWriteValid := true.B
    tptDeadlineAsyncWriteThread := interrupt_thread.U
    tptDeadlineAsyncWriteData := 0.U
    writeSlotPc(interrupt_thread.U, interrupt_base)
    writeSlotValidRunable(interrupt_thread.U, true.B, true.B)
    writeSlotPriority(interrupt_thread.U, interrupt_prior.U)
    writeSlotDeadline(interrupt_thread.U, 0.U)
    eret := false.B

    dbg_sched_valid := true.B
    dbg_sched_cause := 0.U
    dbg_sched_to := interrupt_thread.U

    current_thread := interrupt_thread.U
    slice_left_ticks := residentSlice(interrupt_thread.U)
    io.new_thread_issue_req.valid := true.B
    io.new_thread_issue_req.bits.pc := interrupt_base
    io.new_thread_issue_req.bits.thread := interrupt_thread.U
    
  } .elsewhen (io.xcpt.valid && current_thread === io.xcpt.bits.thread) {

    val switch_to = Mux(do_intr_eret_switch && sched_pick_found, sched_pick_thread, current_thread)

    when (tick_count_fire) {
      when (slice_left_ticks =/= 0.U) {
        slice_left_ticks := slice_left_ticks - 1.U
      }
    }

    when (do_intr_eret_switch) {
      dbg_sched_valid := true.B
      dbg_sched_cause := 1.U
      dbg_sched_to := switch_to
    }

    io.new_thread_issue_req.valid := true.B
    io.new_thread_issue_req.bits.pc := issuePc(switch_to)
    io.new_thread_issue_req.bits.thread := switch_to
    current_thread := switch_to
    when (do_intr_eret_switch) {
      slice_left_ticks := residentSlice(switch_to)
    }

    when (io.xcpt.bits.wb_xcpt && current_thread =/= interrupt_thread.U) {
      layer_xcpt := (layer_xcpt << 1) | 1.U
      tptPriorityAsyncWriteValid := true.B
      tptPriorityAsyncWriteThread := current_thread
      tptPriorityAsyncWriteData := exception_prior.U
      writeSlotPriority(current_thread, exception_prior.U)
      when (layer_xcpt === 0.U) {
        pre_prior := cur_prio
      }
    } .elsewhen (io.xcpt.bits.eret && current_thread =/= interrupt_thread.U) {
      layer_xcpt := (layer_xcpt >> 1)
      when (layer_xcpt === 1.U) {
        tptPriorityAsyncWriteValid := true.B
        tptPriorityAsyncWriteThread := current_thread
        tptPriorityAsyncWriteData := pre_prior
        writeSlotPriority(current_thread, pre_prior)
      }
      random_ctx_time := 1.U
    } .elsewhen(io.xcpt.bits.eret && current_thread === interrupt_thread.U) {
      when (eret) {
        writeSlotRunable(interrupt_thread.U, false.B)
      }
    }

  }. elsewhen (should_switch_yield_halt) {
    // current thread yield/halt
    val switch_id = Mux(sched_pick_found, sched_pick_thread, current_thread)
    val switched = sched_pick_found && (switch_id =/= current_thread)
    when (switched) {
      dbg_sched_valid := true.B
      dbg_sched_cause := 2.U
      dbg_sched_to := switch_id
    }
    current_thread := switch_id
    io.new_thread_issue_req.valid := switched
    io.new_thread_issue_req.bits.pc := issuePc(switch_id)
    io.new_thread_issue_req.bits.thread := switch_id
    when (switched) {
      slice_left_ticks := residentSlice(switch_id)
    }
  }. elsewhen (should_switch_pass) {
    val switch_id = Mux(sched_pick_found, sched_pick_thread, current_thread)
    val switched = sched_pick_found && (switch_id =/= current_thread)
    when (switched) {
      dbg_sched_valid := true.B
      dbg_sched_cause := 3.U
      dbg_sched_to := switch_id
    }
    current_thread := switch_id
    io.new_thread_issue_req.valid := switched
    io.new_thread_issue_req.bits.pc := issuePc(switch_id)
    io.new_thread_issue_req.bits.thread := switch_id
    // PASS is a manual slice boundary: restart the selected threadlet's slice.
    slice_left_ticks := residentSlice(switch_id)
  }. elsewhen (sched_tick_fire) {
    // Periodic scheduler tick (32-cycle)
    when (current_thread =/= interrupt_thread.U) {
      when (!slice_will_expire) {
        when (preempt_hi_found) {
          val switch_id = preempt_hi_thread
          dbg_sched_valid := true.B
          dbg_sched_cause := 4.U
          dbg_sched_to := switch_id
          current_thread := switch_id
          slice_left_ticks := residentSlice(switch_id)
          io.new_thread_issue_req.valid := true.B
          io.new_thread_issue_req.bits.pc := issuePc(switch_id)
          io.new_thread_issue_req.bits.thread := switch_id
        }.otherwise {
          slice_left_ticks := slice_left_ticks - 1.U
        }
      }.otherwise {
        val switch_id = Mux(sched_pick_found, sched_pick_thread, current_thread)
        val switched = sched_pick_found && (switch_id =/= current_thread)
        when (switched) {
          dbg_sched_valid := true.B
          dbg_sched_cause := 5.U
          dbg_sched_to := switch_id
        }
        when (sched_pick_found) {
          current_thread := switch_id
          slice_left_ticks := residentSlice(switch_id)
          io.new_thread_issue_req.valid := switched
          io.new_thread_issue_req.bits.pc := issuePc(switch_id)
          io.new_thread_issue_req.bits.thread := switch_id
        }.otherwise {
          slice_left_ticks := residentSlice(current_thread)
        }
      }
    }
  }

  val coldPcEvictWriteValid = io.slot_load_request.valid
  val coldPcEvictThread = io.slot_thread(io.slot_load_request.bits.slot)
  val coldPcEvictData = slot_pc(io.slot_load_request.bits.slot)
  val coldPcWriteValid = coldPcEvictWriteValid || coldPcCreateWriteValid
  val coldPcWriteThread =
    Mux(coldPcEvictWriteValid, coldPcEvictThread, coldPcCreateWriteThread)
  val coldPcWriteData =
    Mux(coldPcEvictWriteValid, coldPcEvictData, coldPcCreateWriteData)
  val coldPcWriteBank = coldPcWriteThread(pcBankBits - 1, 0)
  val coldPcWriteRow = coldPcWriteThread(threadIdLength - 1, pcBankBits)
  for (bank <- 0 until pcBankCount) {
    when (coldPcWriteValid && coldPcWriteBank === bank.U) {
      coldPcBanks(bank).write(coldPcWriteRow, coldPcWriteData)
    }
  }
  if (coreParams.threadletAreaDebugAssert) {
    assert(!(coldPcEvictWriteValid && coldPcCreateWriteValid),
      "cold PC write port conflict")
  }

  when (tptPriorityWriteValid) {
    tptPriorityMem.write(tptPriorityWriteThread, tptPriorityWriteData)
  }
  when (tptSliceWriteValid) {
    tptSliceMem.write(tptSliceWriteThread, tptSliceWriteData)
  }
  when (tptDeadlineWriteValid) {
    tptDeadlineMem.write(tptDeadlineWriteThread, tptDeadlineWriteData)
  }

  if (coreParams.threadletAreaDebugPrintf) {
    when (dbg_sched_valid) {
      midas.targetutils.SynthesizePrintf(printf(
        "[TM][sched] hart=%d cause=%d from=%d to=%d\n",
        io.hartid, dbg_sched_cause, current_thread, dbg_sched_to))
    }
  }

}
