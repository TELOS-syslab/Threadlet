// See LICENSE.Berkeley for license details.
// See LICENSE.SiFive for license details.

package freechips.rocketchip.rocket

import chisel3._
import chisel3.util._
import chisel3.withClock
import org.chipsalliance.cde.config.Parameters
import freechips.rocketchip.tile._
import freechips.rocketchip.util._
import freechips.rocketchip.util.property
import scala.collection.mutable.ArrayBuffer
import freechips.rocketchip.trace._

case class RocketCoreParams(
  xLen: Int = 64,
  pgLevels: Int = 3, // sv39 default
  bootFreqHz: BigInt = 0,
  useVM: Boolean = true,
  useUser: Boolean = false,
  useSupervisor: Boolean = false,
  useHypervisor: Boolean = false,
  useDebug: Boolean = true,
  useAtomics: Boolean = true,
  useAtomicsOnlyForIO: Boolean = false,
  useCompressed: Boolean = false,
  useRVE: Boolean = false,
  useConditionalZero: Boolean = false,
  useZba: Boolean = false,
  useZbb: Boolean = false,
  useZbs: Boolean = false,
  nLocalInterrupts: Int = 0,
  useNMI: Boolean = false,
  nBreakpoints: Int = 1,
  useBPWatch: Boolean = false,
  mcontextWidth: Int = 0,
  scontextWidth: Int = 0,
  nPMPs: Int = 8,
  nPerfCounters: Int = 0,
  haveBasicCounters: Boolean = true,
  haveCFlush: Boolean = false,
  misaWritable: Boolean = true,
  nL2TLBEntries: Int = 0,
  nL2TLBWays: Int = 1,
  nPTECacheEntries: Int = 8,
  mtvecInit: Option[BigInt] = Some(BigInt(0)),
  mtvecWritable: Boolean = true,
  fastLoadWord: Boolean = true,
  fastLoadByte: Boolean = false,
  branchPredictionModeCSR: Boolean = false,
  clockGate: Boolean = false,
  mvendorid: Int = 0, // 0 means non-commercial implementation
  mimpid: Int = 0x20181004, // release date in BCD
  mulDiv: Option[MulDivParams] = Some(MulDivParams()),
  fpu: Option[FPUParams] = Some(FPUParams()),
  debugROB: Option[DebugROBParams] = None, // if size < 1, SW ROB, else HW ROB
  haveCease: Boolean = true, // non-standard CEASE instruction
  haveSimTimeout: Boolean = true, // add plusarg for simulation timeout
  vector: Option[RocketCoreVectorParams] = None,
  enableTraceCoreIngress: Boolean = false,
  override val threadSupport: Int = 512,
  override val threadSlotCount: Int = 8,
  override val threadletAreaDebugPrintf: Boolean = true,
  override val threadletAreaDebugAssert: Boolean = true
) extends CoreParams {
  val lgPauseCycles = 5
  val haveFSDirty = false
  val pmpGranularity: Int = if (useHypervisor) 4096 else 4
  val fetchWidth: Int = if (useCompressed) 2 else 1
  //  fetchWidth doubled, but coreInstBytes halved, for RVC:
  val decodeWidth: Int = fetchWidth / (if (useCompressed) 2 else 1)
  val retireWidth: Int = 1
  val instBits: Int = if (useCompressed) 16 else 32
  val lrscCycles: Int = 80 // worst case is 14 mispredicted branches + slop
  val traceHasWdata: Boolean = debugROB.isDefined // ooo wb, so no wdata in trace
  override val useVector = vector.isDefined
  override val vectorUseDCache = vector.map(_.useDCache).getOrElse(false)
  override def vLen = vector.map(_.vLen).getOrElse(0)
  override def eLen = vector.map(_.eLen).getOrElse(0)
  override def vfLen = vector.map(_.vfLen).getOrElse(0)
  override def vfh = vector.map(_.vfh).getOrElse(false)
  override def vExts = vector.map(_.vExts).getOrElse(Nil)
  override def vMemDataBits = vector.map(_.vMemDataBits).getOrElse(0)
  override val customIsaExt = Option.when(haveCease)("xrocket") // CEASE instruction
  override def minFLen: Int = fpu.map(_.minFLen).getOrElse(32)
  override def customCSRs(implicit p: Parameters) = new RocketCustomCSRs
}

trait HasRocketCoreParameters extends HasCoreParameters {
  lazy val rocketParams: RocketCoreParams = tileParams.core.asInstanceOf[RocketCoreParams]

  val fastLoadWord = rocketParams.fastLoadWord
  val fastLoadByte = rocketParams.fastLoadByte

  val mulDivParams = rocketParams.mulDiv.getOrElse(MulDivParams())

  require(!fastLoadByte || fastLoadWord)
  require(!rocketParams.haveFSDirty, "rocket doesn't support setting fs dirty from outside, please disable haveFSDirty")
}

class RocketCustomCSRs(implicit p: Parameters) extends CustomCSRs with HasRocketCoreParameters {
  override def bpmCSR = {
    rocketParams.branchPredictionModeCSR.option(CustomCSR(bpmCSRId, BigInt(1), Some(BigInt(0))))
  }

  private def haveDCache = tileParams.dcache.get.scratch.isEmpty

  override def chickenCSR = {
    val mask = BigInt(
      tileParams.dcache.get.clockGate.toInt << 0 |
      rocketParams.clockGate.toInt << 1 |
      rocketParams.clockGate.toInt << 2 |
      1 << 3 | // disableSpeculativeICacheRefill
      haveDCache.toInt << 9 | // suppressCorruptOnGrantData
      tileParams.icache.get.prefetch.toInt << 17
    )
    Some(CustomCSR(chickenCSRId, mask, Some(mask)))
  }

  def disableICachePrefetch = getOrElse(chickenCSR, _.value(17), true.B)

  def marchid = CustomCSR.constant(CSRs.marchid, BigInt(1))

  def mvendorid = CustomCSR.constant(CSRs.mvendorid, BigInt(rocketParams.mvendorid))

  // mimpid encodes a release version in the form of a BCD-encoded datestamp.
  def mimpid = CustomCSR.constant(CSRs.mimpid, BigInt(rocketParams.mimpid))

  override def decls = super.decls :+ marchid :+ mvendorid :+ mimpid
}

class CoreInterrupts(val hasBeu: Boolean)(implicit p: Parameters) extends TileInterrupts()(p) {
  val buserror = Option.when(hasBeu)(Bool())
}

class NICTesterIO(implicit p: Parameters) extends CoreBundle()(p) {
  val valid = Output(Bool())
}

trait HasRocketCoreIO extends HasRocketCoreParameters {
  implicit val p: Parameters
  def nTotalRoCCCSRs: Int
  def traceIngressParams = TraceCoreParams(nGroups = 1, iretireWidth = coreParams.retireWidth, 
                                            xlen = coreParams.xLen, iaddrWidth = coreParams.xLen) 
  val io = IO(new CoreBundle()(p) {
    val hartid = Input(UInt(hartIdLen.W))
    val reset_vector = Input(UInt(resetVectorLen.W))
    val interrupts = Input(new CoreInterrupts(tileParams.asInstanceOf[RocketTileParams].beuAddr.isDefined))
    val imem  = new FrontendIO
    val dmem = new HellaCacheIO
    val ptw = Flipped(new DatapathPTWIO())
    val fpu = Flipped(new FPUCoreIO())
    val rocc = Flipped(new RoCCCoreIO(nTotalRoCCCSRs))
    val trace = Output(new TraceBundle)
    val bpwatch = Output(Vec(coreParams.nBreakpoints, new BPWatch(coreParams.retireWidth)))
    val cease = Output(Bool())
    val wfi = Output(Bool())
    val traceStall = Input(Bool())
    val vector = if (usingVector) Some(Flipped(new VectorCoreIO)) else None
    val trace_core_ingress = if (rocketParams.enableTraceCoreIngress) Some(Output(new TraceCoreInterface(traceIngressParams))) else None
    val nic_tester = new NICTesterIO
  })
}


class Rocket(tile: RocketTile)(implicit p: Parameters) extends CoreModule()(p)
    with HasRocketCoreParameters
    with HasRocketCoreIO {
  def nTotalRoCCCSRs = tile.roccCSRs.flatten.size
  import ALU._

  val clock_en_reg = RegInit(true.B)
  val long_latency_stall = Reg(Bool())
  val id_reg_pause = Reg(Bool())
  val imem_might_request_reg = Reg(Bool())
  val clock_en = WireDefault(true.B)
  val gated_clock =
    if (!rocketParams.clockGate) clock
    else ClockGate(clock, clock_en, "rocket_clock_gate")

  class RocketImpl { // entering gated-clock domain

  val coreDmem = Wire(new HellaCacheIO)

  // performance counters
  def pipelineIDToWB[T <: Data](x: T): T =
    RegEnable(RegEnable(RegEnable(x, !ctrl_killd), ex_pc_valid), mem_pc_valid)
  val perfEvents = new EventSets(Seq(
    new EventSet((mask, hits) => Mux(wb_xcpt, mask(0), wb_valid && pipelineIDToWB((mask & hits).orR)), Seq(
      ("exception", () => false.B),
      ("load", () => id_ctrl.mem && id_ctrl.mem_cmd === M_XRD && !id_ctrl.fp),
      ("store", () => id_ctrl.mem && id_ctrl.mem_cmd === M_XWR && !id_ctrl.fp),
      ("amo", () => usingAtomics.B && id_ctrl.mem && (isAMO(id_ctrl.mem_cmd) || id_ctrl.mem_cmd.isOneOf(M_XLR, M_XSC))),
      ("system", () => id_ctrl.csr =/= CSR.N),
      ("arith", () => id_ctrl.wxd && !(id_ctrl.jal || id_ctrl.jalr || id_ctrl.mem || id_ctrl.fp || id_ctrl.mul || id_ctrl.div || id_ctrl.csr =/= CSR.N)),
      ("branch", () => id_ctrl.branch),
      ("jal", () => id_ctrl.jal),
      ("jalr", () => id_ctrl.jalr))
      ++ (if (!usingMulDiv) Seq() else Seq(
        ("mul", () => if (pipelinedMul) id_ctrl.mul else id_ctrl.div && (id_ctrl.alu_fn & FN_DIV) =/= FN_DIV),
        ("div", () => if (pipelinedMul) id_ctrl.div else id_ctrl.div && (id_ctrl.alu_fn & FN_DIV) === FN_DIV)))
      ++ (if (!usingFPU) Seq() else Seq(
        ("fp load", () => id_ctrl.fp && io.fpu.dec.ldst && io.fpu.dec.wen),
        ("fp store", () => id_ctrl.fp && io.fpu.dec.ldst && !io.fpu.dec.wen),
        ("fp add", () => id_ctrl.fp && io.fpu.dec.fma && io.fpu.dec.swap23),
        ("fp mul", () => id_ctrl.fp && io.fpu.dec.fma && !io.fpu.dec.swap23 && !io.fpu.dec.ren3),
        ("fp mul-add", () => id_ctrl.fp && io.fpu.dec.fma && io.fpu.dec.ren3),
        ("fp div/sqrt", () => id_ctrl.fp && (io.fpu.dec.div || io.fpu.dec.sqrt)),
        ("fp other", () => id_ctrl.fp && !(io.fpu.dec.ldst || io.fpu.dec.fma || io.fpu.dec.div || io.fpu.dec.sqrt))))),
    new EventSet((mask, hits) => (mask & hits).orR, Seq(
      ("load-use interlock", () => id_ex_hazard && ex_ctrl.mem || id_mem_hazard && mem_ctrl.mem || id_wb_hazard && wb_ctrl.mem),
      ("long-latency interlock", () => id_sboard_hazard),
      ("csr interlock", () => id_ex_hazard && ex_ctrl.csr =/= CSR.N || id_mem_hazard && mem_ctrl.csr =/= CSR.N || id_wb_hazard && wb_ctrl.csr =/= CSR.N),
      ("I$ blocked", () => icache_blocked),
      ("D$ blocked", () => id_ctrl.mem && dcache_blocked),
      ("branch misprediction", () => take_pc_mem && mem_direction_misprediction),
      ("control-flow target misprediction", () => take_pc_mem && mem_misprediction && mem_cfi && !mem_direction_misprediction && !icache_blocked),
      ("flush", () => wb_reg_flush_pipe),
      ("replay", () => replay_wb))
      ++ (if (!usingMulDiv) Seq() else Seq(
        ("mul/div interlock", () => id_ex_hazard && (ex_ctrl.mul || ex_ctrl.div) || id_mem_hazard && (mem_ctrl.mul || mem_ctrl.div) || id_wb_hazard && wb_ctrl.div)))
      ++ (if (!usingFPU) Seq() else Seq(
        ("fp interlock", () => id_ex_hazard && ex_ctrl.fp || id_mem_hazard && mem_ctrl.fp || id_wb_hazard && wb_ctrl.fp || id_ctrl.fp && id_stall_fpu)))),
    new EventSet((mask, hits) => (mask & hits).orR, Seq(
      ("I$ miss", () => io.imem.perf.acquire),
      ("D$ miss", () => coreDmem.perf.acquire),
      ("D$ release", () => coreDmem.perf.release),
      ("ITLB miss", () => io.imem.perf.tlbMiss),
      ("DTLB miss", () => coreDmem.perf.tlbMiss),
      ("L2 TLB miss", () => io.ptw.perf.l2miss)))))

  val pipelinedMul = usingMulDiv && mulDivParams.mulUnroll == xLen
  val decode_table = {
    (if (usingMulDiv) new MDecode(pipelinedMul) +: (xLen > 32).option(new M64Decode(pipelinedMul)).toSeq else Nil) ++:
    (if (usingAtomics) new ADecode +: (xLen > 32).option(new A64Decode).toSeq else Nil) ++:
    (if (fLen >= 32)    new FDecode +: (xLen > 32).option(new F64Decode).toSeq else Nil) ++:
    (if (fLen >= 64)    new DDecode +: (xLen > 32).option(new D64Decode).toSeq else Nil) ++:
    (if (minFLen == 16) new HDecode +: (xLen > 32).option(new H64Decode).toSeq ++: (fLen >= 64).option(new HDDecode).toSeq else Nil) ++:
    (usingRoCC.option(new RoCCDecode)) ++:
    (if (xLen == 32) new I32Decode else new I64Decode) +:
    (usingVM.option(new SVMDecode)) ++:
    (usingSupervisor.option(new SDecode)) ++:
    (usingHypervisor.option(new HypervisorDecode)) ++:
    ((usingHypervisor && (xLen == 64)).option(new Hypervisor64Decode)) ++:
    (usingDebug.option(new DebugDecode)) ++:
    (usingNMI.option(new NMIDecode)) ++:
    (usingConditionalZero.option(new ConditionalZeroDecode)) ++:
    Seq(new FenceIDecode(tile.dcache.flushOnFenceI)) ++:
    coreParams.haveCFlush.option(new CFlushDecode(tile.dcache.canSupportCFlushLine)) ++:
    rocketParams.haveCease.option(new CeaseDecode) ++:
    usingVector.option(new VCFGDecode) ++:
    (if (coreParams.useZba) new ZbaDecode +: (xLen > 32).option(new Zba64Decode).toSeq else Nil) ++:
    (if (coreParams.useZbb) Seq(new ZbbDecode, if (xLen == 32) new Zbb32Decode else new Zbb64Decode) else Nil) ++:
    coreParams.useZbs.option(new ZbsDecode) ++:
    // threadlet instruction table
    (if (useMultithreading) Seq(new ThreadDecode) else Nil) ++:
    Seq(new IDecode)
  } flatMap(_.table)

  val ex_ctrl = Reg(new IntCtrlSigs)
  val mem_ctrl = Reg(new IntCtrlSigs)
  val wb_ctrl = Reg(new IntCtrlSigs)

  val ex_thread = Reg(UInt(threadIdLength.W))
  val mem_thread = Reg(UInt(threadIdLength.W))
  val wb_thread = Reg(UInt(threadIdLength.W))

  val ex_slot = Reg(UInt(threadSlotIdLength.W))
  val mem_slot = Reg(UInt(threadSlotIdLength.W))
  val wb_slot = Reg(UInt(threadSlotIdLength.W))

  val ex_thread_ctx_write_slot = Wire(UInt(threadSlotIdLength.W))
  val ex_thread_ctx_write_resident = Wire(Bool())
  val mem_thread_ctx_write_slot = RegInit(0.U(threadSlotIdLength.W))
  val mem_thread_ctx_write_resident = RegInit(false.B)
  val wb_thread_ctx_write_slot = RegInit(0.U(threadSlotIdLength.W))
  val wb_thread_ctx_write_resident = RegInit(false.B)

  // ThreadManager-related operands must be aligned with the EX->MEM pipeline advance.
  val mem_thread_rs0 = RegInit(0.U(xLen.W))
  val mem_thread_rs1 = RegInit(0.U(xLen.W))

  val ex_reg_ctx_switch      = Reg(Bool())
  val ex_reg_xcpt_interrupt  = Reg(Bool())
  val ex_reg_valid           = Reg(Bool())
  val ex_reg_rvc             = Reg(Bool())
  val ex_reg_btb_resp        = Reg(new BTBResp)
  val ex_reg_xcpt            = Reg(Bool())
  val ex_reg_flush_pipe      = Reg(Bool())
  val ex_reg_load_use        = Reg(Bool())
  val ex_reg_cause           = Reg(UInt())
  val ex_reg_replay = Reg(Bool())
  val ex_reg_pc = Reg(UInt())
  val ex_reg_mem_size = Reg(UInt())
  val ex_reg_hls = Reg(Bool())
  val ex_reg_inst = Reg(Bits())
  val ex_reg_raw_inst = Reg(UInt())
  val ex_reg_wphit            = Reg(Vec(nBreakpoints, Bool()))
  val ex_reg_set_vconfig      = Reg(Bool())

  val mem_reg_xcpt_interrupt  = Reg(Bool())
  val mem_reg_valid           = RegInit(false.B)
  val mem_reg_rvc             = Reg(Bool())
  val mem_reg_btb_resp        = Reg(new BTBResp)
  val mem_reg_xcpt            = Reg(Bool())
  val mem_reg_replay          = Reg(Bool())
  val mem_reg_flush_pipe      = Reg(Bool())
  val mem_reg_cause           = Reg(UInt())
  val mem_reg_slow_bypass     = Reg(Bool())
  val mem_reg_load            = Reg(Bool())
  val mem_reg_store           = Reg(Bool())
  val mem_reg_set_vconfig     = Reg(Bool())
  val mem_reg_sfence = Reg(Bool())
  val mem_reg_pc = Reg(UInt())
  val mem_reg_inst = Reg(Bits())
  val mem_reg_mem_size = Reg(UInt())
  val mem_reg_hls_or_dv = Reg(Bool())
  val mem_reg_raw_inst = Reg(UInt())
  val mem_reg_wdata = Reg(Bits())
  val mem_reg_rs2 = Reg(Bits())
  val mem_br_taken = Reg(Bool())
  val take_pc_mem = Wire(Bool())
  val mem_reg_wphit          = Reg(Vec(nBreakpoints, Bool()))
  val mem_reg_test = RegInit(false.B)

  val wb_reg_ctx_switch      = Reg(Bool())
  val wb_reg_valid           = Reg(Bool())
  val wb_reg_xcpt            = Reg(Bool())
  val wb_reg_xcpt_interrupt  = RegInit(false.B)
  val wb_reg_replay          = Reg(Bool())
  val wb_reg_flush_pipe      = Reg(Bool())
  val wb_reg_cause           = Reg(UInt())
  val wb_reg_set_vconfig     = Reg(Bool())
  val wb_reg_sfence = Reg(Bool())
  val wb_reg_pc = Reg(UInt())
  val wb_reg_mem_size = Reg(UInt())
  val wb_reg_hls_or_dv = Reg(Bool())
  val wb_reg_hfence_v = Reg(Bool())
  val wb_reg_hfence_g = Reg(Bool())
  val wb_reg_inst = Reg(Bits())
  val wb_reg_raw_inst = Reg(UInt())
  val wb_reg_wdata = Reg(Bits())
  val wb_reg_rs2 = Reg(Bits())
  val wb_reg_br_taken = Reg(Bool())
  val take_pc_wb = Wire(Bool())
  val wb_reg_wphit           = Reg(Vec(nBreakpoints, Bool()))
  val first      = RegInit(true.B)
  first := first && !mem_reg_valid

  val interrupt_threadlet_mode = RegInit(false.B)

  val take_pc_mem_wb = take_pc_wb || take_pc_mem
  val take_pc = take_pc_mem_wb
  val thread_yield = Wire(Bool())

  // checking whether the stage need to be cleared when encountering exceptions/interrupts
  def needToClear(thread_of_stage: UInt) = (take_pc_wb && wb_thread === thread_of_stage) || (take_pc_mem && mem_thread === thread_of_stage) || (thread_yield && mem_thread === thread_of_stage)
  
  // decode stage
  val ibuf = Module(new IBuf)
  val id_expanded_inst = ibuf.io.inst.map(_.bits.inst)
  val id_raw_inst = ibuf.io.inst.map(_.bits.raw)
  val id_inst = id_expanded_inst.map(_.bits)
  ibuf.io.imem <> io.imem.resp

  val id_thread = ibuf.io.thread
  val id_valid = ibuf.io.inst(0).valid
  ibuf.io.kill := (if (useMultithreading) false.B else take_pc)

  require(decodeWidth == 1            && retireWidth == decodeWidth)
  require(!(coreParams.useRVE && coreParams.fpu.nonEmpty), "Can't select both RVE and floating-point")
  require(!(coreParams.useRVE && coreParams.useHypervisor), "Can't select both RVE and Hypervisor")
  val id_ctrl = Wire(new IntCtrlSigs).decode(id_inst(0), decode_table)

  val lgNXRegs = if (coreParams.useRVE) 4 else 5
  val regAddrMask = (1 << lgNXRegs) - 1
  val rfAddrBits = log2Ceil((1 << lgNXRegs) * threadSlotCount)

  val tm = Module(new ThreadManager)
  val threadCtxMgr = if (useMultithreading) Some(Module(new ThreadContextManager)) else None
  val threadCtxSaver = if (useMultithreading) Some(Module(new ThreadContextSaver)) else None
  val threadCtxDmemArb = if (useMultithreading) Some(Module(new ThreadContextDmemArbiter)) else None
  val threadlet_ctx_base = if (useMultithreading) Some(RegInit(0.U(paddrBits.W))) else None
  val threadlet_ctx_base_valid = if (useMultithreading) Some(RegInit(false.B)) else None
  val ctxSlotDrainBusy = WireDefault(0.U(threadSlotCount.W))
  val ctxSlotDirtySet = Wire(Valid(UInt(threadSlotIdLength.W)))
  ctxSlotDirtySet.valid := false.B
  ctxSlotDirtySet.bits := 0.U
  val ctxThreadKill = Wire(Valid(UInt(threadIdLength.W)))
  ctxThreadKill.valid := false.B
  ctxThreadKill.bits := 0.U

  def threadSlotHits(thread: UInt): UInt = {
    if (useMultithreading) {
      VecInit((0 until threadSlotCount).map(s =>
        threadCtxMgr.get.io.slot_thread(s) === thread)).asUInt
    } else {
      1.U(1.W)
    }
  }
  def threadResident(thread: UInt): Bool = {
    if (useMultithreading) threadSlotHits(thread).orR else true.B
  }
  def threadToSlot(thread: UInt): UInt = {
    if (useMultithreading) {
      val hits = threadSlotHits(thread)
      if (threadSlotCount == 1) 0.U(threadSlotIdLength.W)
      else Mux(hits.orR, PriorityEncoder(hits), 0.U(threadSlotIdLength.W))
    } else {
      0.U(threadSlotIdLength.W)
    }
  }
  def rfAddr(slot: UInt, reg: UInt): UInt =
    if (threadSlotCount == 1) reg(lgNXRegs - 1, 0) else Cat(slot, reg(lgNXRegs - 1, 0))
  def rfAddrReg(addr: UInt): UInt = addr(lgNXRegs - 1, 0)
  def rfAddrIsX0(addr: UInt): Bool = rfAddrReg(addr) === 0.U
  def decodeReg(x: UInt, thread: UInt) =
    (x.extract(x.getWidth - 1, lgNXRegs).asBool, rfAddr(threadToSlot(thread), x))
  val (id_raddr3_illegal, id_raddr3) = decodeReg(id_expanded_inst(0).rs3, id_thread)
  val (id_raddr2_illegal, id_raddr2) = decodeReg(id_expanded_inst(0).rs2, id_thread)
  val (id_raddr1_illegal, id_raddr1) = decodeReg(id_expanded_inst(0).rs1, id_thread)
  val (id_waddr_illegal,  id_waddr)  = decodeReg(id_expanded_inst(0).rd,  id_thread)

  val id_load_use = Wire(Bool())
  val id_reg_fence = RegInit(false.B)
  val id_ren = IndexedSeq(id_ctrl.rxs1, id_ctrl.rxs2)
  val id_raddr = IndexedSeq(id_raddr1, id_raddr2)
  val rf = new RegFile(1 << lgNXRegs, xLen, false, threadSlotCount)
  val id_rs = id_raddr.map(rf.read _)
  val ctrl_killd = Wire(Bool())
  val id_npc = (ibuf.io.pc.asSInt + ImmGen(IMM_UJ, id_inst(0))).asUInt

  val csr = Module(new CSRFile(perfEvents, coreParams.customCSRs.decls, tile.roccCSRs.flatten, tile.rocketParams.beuAddr.isDefined))
  val id_csr_en = id_ctrl.csr.isOneOf(CSR.S, CSR.C, CSR.W)
  val id_system_insn = id_ctrl.csr === CSR.I
  val id_csr_ren = id_ctrl.csr.isOneOf(CSR.S, CSR.C) && id_expanded_inst(0).rs1 === 0.U
  val id_csr = Mux(id_system_insn && id_ctrl.mem, CSR.N, Mux(id_csr_ren, CSR.R, id_ctrl.csr))
  val id_csr_flush = id_system_insn || (id_csr_en && !id_csr_ren && csr.io.decode(0).write_flush)
  val id_set_vconfig = Seq(Instructions.VSETVLI, Instructions.VSETIVLI, Instructions.VSETVL).map(_ === id_inst(0)).orR && usingVector.B

  id_ctrl.vec := false.B
  if (usingVector) {
    val v_decode = rocketParams.vector.get.decoder(p)
    v_decode.io.inst := id_inst(0)
    v_decode.io.vconfig := csr.io.vector.get.vconfig
    id_ctrl.vec := v_decode.io.vector
    when (v_decode.io.legal) {
      id_ctrl.legal := !csr.io.vector.get.vconfig.vtype.vill
      id_ctrl.fp := v_decode.io.fp
      id_ctrl.rocc := false.B
      id_ctrl.branch := false.B
      id_ctrl.jal := false.B
      id_ctrl.jalr := false.B
      id_ctrl.rxs2 := v_decode.io.read_rs2
      id_ctrl.rxs1 := v_decode.io.read_rs1
      id_ctrl.mem := false.B
      id_ctrl.rfs1 := v_decode.io.read_frs1
      id_ctrl.rfs2 := false.B
      id_ctrl.rfs3 := false.B
      id_ctrl.wfd := v_decode.io.write_frd
      id_ctrl.mul := false.B
      id_ctrl.div := false.B
      id_ctrl.wxd := v_decode.io.write_rd
      id_ctrl.csr := CSR.N
      id_ctrl.fence_i := false.B
      id_ctrl.fence := false.B
      id_ctrl.amo := false.B
      id_ctrl.dp := false.B
      id_ctrl.vec := true.B
    }
  }

  if (useMultithreading) {
    val thread_decode_table = { Seq(new ThreadDecodeSelf) } flatMap(_.table)
    val thread_ctrl = Wire(new ThreadInstCtrlSigs).decode(id_inst(0), thread_decode_table)
    id_ctrl.thread := thread_ctrl.ctrl
  } else {
    id_ctrl.thread.legal := false.B
  }

  val id_illegal_insn = !id_ctrl.legal ||
    (id_ctrl.mul || id_ctrl.div) && !csr.io.status.isa('m'-'a') ||
    id_ctrl.amo && !csr.io.status.isa('a'-'a') ||
    id_ctrl.fp && (csr.io.decode(0).fp_illegal || (io.fpu.illegal_rm && !id_ctrl.vec)) ||
    id_set_vconfig && csr.io.decode(0).vector_illegal ||
    id_ctrl.vec && (csr.io.decode(0).vector_illegal || csr.io.vector.map(_.vconfig.vtype.vill).getOrElse(false.B)) ||
    id_ctrl.dp && !csr.io.status.isa('d'-'a') ||
    ibuf.io.inst(0).bits.rvc && !csr.io.status.isa('c'-'a') ||
    id_raddr2_illegal && id_ctrl.rxs2 ||
    id_raddr1_illegal && id_ctrl.rxs1 ||
    id_waddr_illegal && id_ctrl.wxd ||
    id_ctrl.rocc && csr.io.decode(0).rocc_illegal ||
    id_csr_en && (csr.io.decode(0).read_illegal || !id_csr_ren && csr.io.decode(0).write_illegal) ||
    !ibuf.io.inst(0).bits.rvc && (id_system_insn && csr.io.decode(0).system_illegal)
  val id_virtual_insn = id_ctrl.legal &&
    ((id_csr_en && !(!id_csr_ren && csr.io.decode(0).write_illegal) && csr.io.decode(0).virtual_access_illegal) ||
     (!ibuf.io.inst(0).bits.rvc && id_system_insn && csr.io.decode(0).virtual_system_illegal))
  // stall decode for fences (now, for AMO.rl; later, for AMO.aq and FENCE)
  val id_amo_aq = id_inst(0)(26)
  val id_amo_rl = id_inst(0)(25)
  val id_fence_pred = id_inst(0)(27,24)
  val id_fence_succ = id_inst(0)(23,20)
  val id_fence_next = id_ctrl.fence || id_ctrl.amo && id_amo_aq
  val id_mem_busy = !coreDmem.ordered || coreDmem.req.valid
  when (!id_mem_busy) { id_reg_fence := false.B }
  val id_rocc_busy = usingRoCC.B &&
    (io.rocc.busy || ex_reg_valid && ex_ctrl.rocc ||
     mem_reg_valid && mem_ctrl.rocc || wb_reg_valid && wb_ctrl.rocc)
  val id_csr_rocc_write = tile.roccCSRs.flatten.map(_.id.U === id_inst(0)(31,20)).orR && id_csr_en && !id_csr_ren
  val id_vec_busy = io.vector.map(v => v.backend_busy || v.trap_check_busy).getOrElse(false.B)
  val id_do_fence = WireDefault(id_rocc_busy && (id_ctrl.fence || id_csr_rocc_write) ||
    id_vec_busy && id_ctrl.fence ||
    id_mem_busy && (id_ctrl.amo && id_amo_rl || id_ctrl.fence_i || id_reg_fence && (id_ctrl.mem || id_ctrl.rocc)))

  val bpu = Module(new BreakpointUnit(nBreakpoints))
  bpu.io.status := csr.io.status
  bpu.io.bp := csr.io.bp
  bpu.io.pc := ibuf.io.pc
  bpu.io.ea := mem_reg_wdata
  bpu.io.mcontext := csr.io.mcontext
  bpu.io.scontext := csr.io.scontext

  val preWFI = RegInit(false.B)
  when (csr.io.status.wfi) {
    preWFI := true.B
  } .elsewhen (!csr.io.interrupt) {
    preWFI := false.B
  }

  val print_enable = RegInit(false.B)
  val dcache_print_enable = RegInit(false.B)

  val intr_print_active = RegInit(false.B)
  val intr_wait_switch = RegInit(false.B)
  // Avoid second redirect for threadlet-handled S-mode interrupts; keep M-mode path unchanged.
  val should_redirect = mem_reg_test && !(interrupt_threadlet_mode && csr.io.interrupt_deleg)
  val s_interrupt = csr.io.interrupt && csr.io.interrupt_deleg
  val interrupt_to_this = csr.io.interrupt && (preWFI || !should_redirect || !csr.io.interrupt_deleg)

  when (!intr_print_active && print_enable && csr.io.interrupt) {
    intr_print_active := true.B
    intr_wait_switch := false.B
  }

  val id_interrupt_redirect = s_interrupt && !preWFI && should_redirect
  val id_interrupt_redirect_cause = csr.io.interrupt_cause
  val id_evec = csr.io.evec

  val ex_interrupt_redirect = RegInit(false.B)
  val ex_interrupt_redirect_cause = RegNext(id_interrupt_redirect_cause)
  val ex_evec = RegNext(id_evec)

  val mem_interrupt_redirect = RegInit(false.B)
  val mem_interrupt_redirect_cause = RegNext(ex_interrupt_redirect_cause)
  val mem_evec = RegNext(ex_evec)

  val wb_interrupt_redirect = RegInit(false.B)
  val wb_interrupt_redirect_cause = RegNext(mem_interrupt_redirect_cause)
  val wb_evec = RegNext(mem_evec)

  val interrupt_redirect_clear = Wire(Bool())
  val interrupt_print_debug = RegInit(false.B)
  when (interrupt_threadlet_mode) {
    interrupt_print_debug := true.B
  }

  interrupt_redirect_clear := wb_interrupt_redirect
  ex_interrupt_redirect := !interrupt_redirect_clear && id_interrupt_redirect
  mem_interrupt_redirect := !interrupt_redirect_clear && ex_interrupt_redirect
  wb_interrupt_redirect := !interrupt_redirect_clear && mem_interrupt_redirect

  val id_xcpt0 = ibuf.io.inst(0).bits.xcpt0
  val id_xcpt1 = ibuf.io.inst(0).bits.xcpt1
  val (id_xcpt, id_cause) = checkExceptions(List(
    (interrupt_to_this, csr.io.interrupt_cause),
    (bpu.io.debug_if,  CSR.debugTriggerCause.U),
    (bpu.io.xcpt_if,   Causes.breakpoint.U),
    (id_xcpt0.pf.inst, Causes.fetch_page_fault.U),
    (id_xcpt0.gf.inst, Causes.fetch_guest_page_fault.U),
    (id_xcpt0.ae.inst, Causes.fetch_access.U),
    (id_xcpt1.pf.inst, Causes.fetch_page_fault.U),
    (id_xcpt1.gf.inst, Causes.fetch_guest_page_fault.U),
    (id_xcpt1.ae.inst, Causes.fetch_access.U),
    (id_virtual_insn,  Causes.virtual_instruction.U),
    (id_illegal_insn,  Causes.illegal_instruction.U)))

  val idCoverCauses = List(
    (CSR.debugTriggerCause, "DEBUG_TRIGGER"),
    (Causes.breakpoint, "BREAKPOINT"),
    (Causes.fetch_access, "FETCH_ACCESS"),
    (Causes.illegal_instruction, "ILLEGAL_INSTRUCTION")
  ) ++ (if (usingVM) List(
    (Causes.fetch_page_fault, "FETCH_PAGE_FAULT")
  ) else Nil)
  coverExceptions(id_xcpt, id_cause, "DECODE", idCoverCauses)

  val dcache_bypass_data =
    if (fastLoadByte) coreDmem.resp.bits.data(xLen-1, 0)
    else if (fastLoadWord) coreDmem.resp.bits.data_word_bypass(xLen-1, 0)
    else wb_reg_wdata

  // detect bypass opportunities
  val ex_thread_ctx_write_drop = ex_ctrl.thread.legal &&
    ex_ctrl.thread.ctx_write &&
    !ex_thread_ctx_write_resident
  val mem_thread_ctx_write_drop = mem_ctrl.thread.legal &&
    mem_ctrl.thread.ctx_write &&
    !mem_thread_ctx_write_resident
  val ctxWriteInFlight =
    (ex_reg_valid && ex_ctrl.thread.legal && ex_ctrl.thread.ctx_write) ||
    (mem_reg_valid && mem_ctrl.thread.legal && mem_ctrl.thread.ctx_write) ||
    (wb_reg_valid && wb_ctrl.thread.legal && wb_ctrl.thread.ctx_write)
  val ex_waddr : UInt = Mux(ex_reg_valid && ex_ctrl.thread.legal && ex_ctrl.thread.ctx_write, 
                      rfAddr(ex_thread_ctx_write_slot, ex_reg_inst(11, 7) & regAddrMask.U),
                      rfAddr(ex_slot, ex_reg_inst(11, 7) & regAddrMask.U))
  val mem_waddr : UInt = Mux(mem_reg_valid && mem_ctrl.thread.legal && mem_ctrl.thread.ctx_write, 
                      rfAddr(mem_thread_ctx_write_slot, mem_reg_inst(11, 7) & regAddrMask.U),
                      rfAddr(mem_slot, mem_reg_inst(11, 7) & regAddrMask.U))
  val wb_waddr : UInt = Mux(wb_reg_valid && wb_ctrl.thread.legal && wb_ctrl.thread.ctx_write, 
                      rfAddr(wb_thread_ctx_write_slot, wb_reg_inst(11, 7) & regAddrMask.U),
                      rfAddr(wb_slot, wb_reg_inst(11, 7) & regAddrMask.U))

  val bypass_zero: IndexedSeq[(Bool, UInt, UInt)] = (1 until threadSlotCount).foldLeft(IndexedSeq((true.B, 0.U, 0.U)).asInstanceOf[IndexedSeq[(Bool, UInt, UInt)]]) ( (c, i) =>
    // treat reading x0 as a bypass
    c ++ IndexedSeq((true.B, rfAddr(i.U(threadSlotIdLength.W), 0.U(lgNXRegs.W)), 0.U)).asInstanceOf[IndexedSeq[(Bool, UInt, UInt)]]
  )

  val bypass_sources_without_x0: IndexedSeq[(Bool, UInt, UInt)] = IndexedSeq(
    (ex_reg_valid && ex_ctrl.wxd && ex_ctrl.thread.legal && ex_ctrl.thread.create, ex_waddr, tm.io.ret.bits.thread),
    (ex_reg_valid && ex_ctrl.wxd && ex_ctrl.thread.legal && ex_ctrl.thread.current, ex_waddr, mem_thread),
    (ex_reg_valid && ex_ctrl.wxd && !ex_thread_ctx_write_drop, ex_waddr, mem_reg_wdata),
    (mem_reg_valid && mem_ctrl.wxd && !mem_ctrl.mem && !mem_thread_ctx_write_drop, mem_waddr, wb_reg_wdata),
    (mem_reg_valid && mem_ctrl.wxd && !mem_thread_ctx_write_drop, mem_waddr, dcache_bypass_data))

  val bypass_sources: IndexedSeq[(Bool, UInt, UInt)] = bypass_zero ++ bypass_sources_without_x0
  val id_bypass_src = id_raddr.map(raddr => bypass_sources.map(s => s._1 && s._2 === raddr))

  // execute stage
  val bypass_mux = bypass_sources.map(_._3)
  val ex_reg_rs_bypass = Reg(Vec(id_raddr.size, Bool()))
  val ex_reg_rs_lsb = Reg(Vec(id_raddr.size, UInt(log2Ceil(bypass_sources.size).W)))
  val ex_reg_rs_msb = Reg(Vec(id_raddr.size, UInt()))
  val ex_rs = for (i <- 0 until id_raddr.size)
    yield Mux(ex_reg_rs_bypass(i), bypass_mux(ex_reg_rs_lsb(i)), Cat(ex_reg_rs_msb(i), ex_reg_rs_lsb(i)))
  val ex_imm = ImmGen(ex_ctrl.sel_imm, ex_reg_inst)
  val ex_rs1shl = Mux(ex_reg_inst(3), ex_rs(0)(31,0), ex_rs(0)) << ex_reg_inst(14,13)
  val ex_op1 = MuxLookup(ex_ctrl.sel_alu1, 0.S)(Seq(
    A1_RS1 -> ex_rs(0).asSInt,
    A1_PC -> ex_reg_pc.asSInt,
    A1_RS1SHL -> (if (rocketParams.useZba) ex_rs1shl.asSInt else 0.S)
  ))
  val ex_op2_oh = UIntToOH(Mux(ex_ctrl.sel_alu2(0), (ex_reg_inst >> 20).asUInt, ex_rs(1))(log2Ceil(xLen)-1,0)).asSInt
  val ex_op2 = MuxLookup(ex_ctrl.sel_alu2, 0.S)(Seq(
    A2_RS2 -> ex_rs(1).asSInt,
    A2_IMM -> ex_imm,
    A2_SIZE -> Mux(ex_reg_rvc, 2.S, 4.S),
  ) ++ (if (coreParams.useZbs) Seq(
    A2_RS2OH -> ex_op2_oh,
    A2_IMMOH -> ex_op2_oh,
  ) else Nil))

  val (ex_new_vl, ex_new_vconfig) = if (usingVector) {
    val ex_new_vtype = VType.fromUInt(MuxCase(ex_rs(1), Seq(
      ex_reg_inst(31,30).andR -> ex_reg_inst(29,20),
      !ex_reg_inst(31)        -> ex_reg_inst(30,20))))
    val ex_avl = Mux(ex_ctrl.rxs1,
      Mux(ex_reg_inst(19,15) === 0.U,
        Mux(ex_reg_inst(11,7) === 0.U, csr.io.vector.get.vconfig.vl, ex_new_vtype.vlMax),
        ex_rs(0)
      ),
      ex_reg_inst(19,15))
    val ex_new_vl = ex_new_vtype.vl(ex_avl, csr.io.vector.get.vconfig.vl, false.B, false.B, false.B)
    val ex_new_vconfig = Wire(new VConfig)
    ex_new_vconfig.vtype := ex_new_vtype
    ex_new_vconfig.vl := ex_new_vl
    (Some(ex_new_vl), Some(ex_new_vconfig))
  } else { (None, None) }

  val alu = Module(new ALU)
  alu.io.dw := ex_ctrl.alu_dw
  alu.io.fn := ex_ctrl.alu_fn
  alu.io.in2 := ex_op2.asUInt
  alu.io.in1 := ex_op1.asUInt

  // multiplier and divider
  val nIntPhysRegs = 32 * threadSlotCount
  val div = Module(new MulDiv(
    if (pipelinedMul) mulDivParams.copy(mulUnroll = 0) else mulDivParams,
    width = xLen,
    nXpr = nIntPhysRegs))
  div.io.req.valid := ex_reg_valid && ex_ctrl.div
  div.io.req.bits.dw := ex_ctrl.alu_dw
  div.io.req.bits.fn := ex_ctrl.alu_fn
  div.io.req.bits.in1 := ex_rs(0)
  div.io.req.bits.in2 := ex_rs(1)
  div.io.req.bits.tag := ex_waddr
  val mul = pipelinedMul.option {
    val m = Module(new PipelinedMultiplier(xLen, 2, nXpr = nIntPhysRegs))
    m.io.req.valid := ex_reg_valid && ex_ctrl.mul
    m.io.req.bits := div.io.req.bits
    m
  }

  ex_reg_valid := !ctrl_killd
  ex_reg_replay := !needToClear(id_thread) && id_valid && ibuf.io.inst(0).bits.replay
  ex_reg_xcpt := !ctrl_killd && id_xcpt
  ex_reg_xcpt_interrupt := !needToClear(id_thread) && id_valid && interrupt_to_this
  when (!ctrl_killd || (id_valid && ibuf.io.inst(0).bits.replay)) { 
    ex_thread := id_thread
    ex_slot := threadToSlot(id_thread)
  }

  tm.io.ret.ready := true.B

  when (!ctrl_killd) {
    ex_ctrl := id_ctrl
    ex_reg_rvc := ibuf.io.inst(0).bits.rvc
    ex_ctrl.csr := id_csr
    when (id_ctrl.fence && id_fence_succ === 0.U) { id_reg_pause := true.B }
    when (id_fence_next) { id_reg_fence := true.B }
    when (id_xcpt) { // pass PC down ALU writeback pipeline for badaddr
      ex_ctrl.alu_fn := FN_ADD
      ex_ctrl.alu_dw := DW_XPR
      ex_ctrl.sel_alu1 := A1_RS1 // badaddr := instruction
      ex_ctrl.sel_alu2 := A2_ZERO
      when (id_xcpt1.asUInt.orR) { // badaddr := PC+2
        ex_ctrl.sel_alu1 := A1_PC
        ex_ctrl.sel_alu2 := A2_SIZE
        ex_reg_rvc := true.B
      }
      when (bpu.io.xcpt_if || id_xcpt0.asUInt.orR) { // badaddr := PC
        ex_ctrl.sel_alu1 := A1_PC
        ex_ctrl.sel_alu2 := A2_ZERO
      }
    }
    ex_reg_flush_pipe := id_ctrl.fence_i || id_csr_flush
    ex_reg_load_use := id_load_use
    ex_reg_hls := usingHypervisor.B && id_system_insn && id_ctrl.mem_cmd.isOneOf(M_XRD, M_XWR, M_HLVX)
    ex_reg_mem_size := Mux(usingHypervisor.B && id_system_insn, id_inst(0)(27, 26), id_inst(0)(13, 12))
    when (id_ctrl.mem_cmd.isOneOf(M_SFENCE, M_HFENCEV, M_HFENCEG, M_FLUSH_ALL)) {
      ex_reg_mem_size := Cat(id_raddr2(lgNXRegs-1, 0) =/= 0.U, id_raddr1(lgNXRegs-1, 0) =/= 0.U)
    }
    when (id_ctrl.mem_cmd === M_SFENCE && csr.io.status.v) {
      ex_ctrl.mem_cmd := M_HFENCEV
    }
    if (tile.dcache.flushOnFenceI) {
      when (id_ctrl.fence_i) {
        ex_reg_mem_size := 0.U
      }
    }

    for (i <- 0 until id_raddr.size) {
      val do_bypass = id_bypass_src(i).reduce(_||_)
      val bypass_src = PriorityEncoder(id_bypass_src(i))
      ex_reg_rs_bypass(i) := do_bypass
      ex_reg_rs_lsb(i) := bypass_src
      when (id_ren(i) && !do_bypass) {
        ex_reg_rs_lsb(i) := id_rs(i)(log2Ceil(bypass_sources.size)-1, 0)
        ex_reg_rs_msb(i) := id_rs(i) >> log2Ceil(bypass_sources.size)
      }
    }
    when (id_illegal_insn || id_virtual_insn) {
      val inst = Mux(ibuf.io.inst(0).bits.rvc, id_raw_inst(0)(15, 0), id_raw_inst(0))
      ex_reg_rs_bypass(0) := false.B
      ex_reg_rs_lsb(0) := inst(log2Ceil(bypass_sources.size)-1, 0)
      ex_reg_rs_msb(0) := inst >> log2Ceil(bypass_sources.size)
    }
  }
  when (!ctrl_killd || interrupt_to_this || ibuf.io.inst(0).bits.replay) {
    ex_reg_cause := id_cause
    ex_reg_inst := id_inst(0)
    ex_reg_raw_inst := id_raw_inst(0)
    ex_reg_pc := ibuf.io.pc
    ex_reg_btb_resp := ibuf.io.btb_resp
    ex_reg_wphit := bpu.io.bpwatch.map { bpw => bpw.ivalid(0) }
    ex_reg_set_vconfig := id_set_vconfig && !id_xcpt
  }

  // replay inst in ex stage?
  val ex_pc_valid = ex_reg_valid || ex_reg_replay || ex_reg_xcpt_interrupt || ex_reg_ctx_switch
  val wb_dcache_miss = wb_ctrl.mem && !coreDmem.resp.valid
  val ex_thread_ctx_write_target = ex_rs(0)(threadIdLength - 1, 0)
  val ex_thread_ctx_write_target_resident = threadResident(ex_thread_ctx_write_target)
  val ex_thread_ctx_write_target_slot = threadToSlot(ex_thread_ctx_write_target)
  val ex_thread_ctx_write_target_locked =
    if (useMultithreading) {
      ex_thread_ctx_write_target_resident &&
        VecInit(threadCtxMgr.get.io.slot_ctx_write_blocked.asBools)(ex_thread_ctx_write_target_slot)
    } else {
      false.B
    }
  val replay_ex_structural = ex_ctrl.mem && !coreDmem.req.ready ||
                             ex_ctrl.div && !div.io.req.ready ||
                             ex_ctrl.vec && !io.vector.map(_.ex.ready).getOrElse(true.B) ||
                             ex_ctrl.thread.legal && ex_ctrl.thread.ctx_write && ex_thread_ctx_write_target_locked
  val replay_ex_load_use = wb_dcache_miss && ex_reg_load_use
  val replay_ex = ex_reg_replay || (ex_reg_valid && (replay_ex_structural || replay_ex_load_use))
  val ctrl_killx = needToClear(ex_thread) || replay_ex || !ex_reg_valid
  // detect 2-cycle load-use delay for LB/LH/SC
  val ex_slow_bypass = ex_ctrl.mem_cmd === M_XSC || ex_reg_mem_size < 2.U
  val ex_sfence = usingVM.B && ex_ctrl.mem && (ex_ctrl.mem_cmd === M_SFENCE || ex_ctrl.mem_cmd === M_HFENCEV || ex_ctrl.mem_cmd === M_HFENCEG)

  val (ex_xcpt, ex_cause) = checkExceptions(List(
    (ex_reg_xcpt_interrupt || ex_reg_xcpt, ex_reg_cause)))

  val exCoverCauses = idCoverCauses
  coverExceptions(ex_xcpt, ex_cause, "EXECUTE", exCoverCauses)

  ex_thread_ctx_write_slot := ex_thread_ctx_write_target_slot
  ex_thread_ctx_write_resident := ex_thread_ctx_write_target_resident

  // memory stage
  val mem_thread_current_wdata = mem_thread
  val mem_thread_ctx_write_wdata = mem_thread_rs1
  
  val mem_thread_wdata = Mux(mem_ctrl.thread.current, 
    mem_thread_current_wdata,
    Mux(mem_ctrl.thread.ctx_write,
    mem_thread_ctx_write_wdata,
    0.U))

  val mem_thread_ctrl_valid = mem_reg_valid && mem_ctrl.thread.legal

  val mem_pc_valid = mem_reg_valid || mem_reg_replay || mem_reg_xcpt_interrupt
  val mem_br_target = mem_reg_pc.asSInt +
    Mux(mem_ctrl.branch && mem_br_taken, ImmGen(IMM_SB, mem_reg_inst),
    Mux(mem_ctrl.jal, ImmGen(IMM_UJ, mem_reg_inst),
    Mux(mem_reg_rvc, 2.S, 4.S)))
  val mem_npc = (Mux(mem_ctrl.jalr || mem_reg_sfence, encodeVirtualAddress(mem_reg_wdata, mem_reg_wdata).asSInt, mem_br_target) & (-2).S).asUInt
  val mem_correct_pc = tm.io.mem_pc
  val mem_wrong_npc =
    if (useMultithreading) Mux(first, false.B, mem_correct_pc =/= mem_reg_pc)
    else Mux(ex_pc_valid, mem_npc =/= ex_reg_pc,
          Mux(ibuf.io.inst(0).valid || ibuf.io.imem.valid, mem_npc =/= ibuf.io.pc, true.B))
  val mem_npc_misaligned = !csr.io.status.isa('c'-'a') && mem_npc(1) && !mem_reg_sfence

  if (coreParams.threadletAreaDebugAssert) {
    assert((tm.io.ret.fire && mem_thread_ctrl_valid && mem_ctrl.thread.create)
      || !tm.io.ret.fire, "mem_ctrl state error")
  }
  val mem_tm_wdata =
    Mux(mem_thread_ctrl_valid && mem_ctrl.thread.create, tm.io.ret.bits.thread, 0.U)
  val mem_int_wdata = Mux(mem_reg_valid && mem_ctrl.thread.legal, 
    Mux(tm.io.ret.fire, mem_tm_wdata, mem_thread_wdata),  
    Mux(!mem_reg_xcpt && (mem_ctrl.jalr ^ mem_npc_misaligned), mem_br_target, mem_reg_wdata.asSInt).asUInt)
  val mem_cfi = mem_ctrl.branch || mem_ctrl.jalr || mem_ctrl.jal
  val mem_cfi_taken = (mem_ctrl.branch && mem_br_taken) || mem_ctrl.jalr || mem_ctrl.jal
  val mem_direction_misprediction = mem_ctrl.branch && mem_br_taken =/= (usingBTB.B && mem_reg_btb_resp.taken)
  val mem_misprediction = if (usingBTB || useMultithreading) mem_wrong_npc else mem_cfi_taken
  take_pc_mem := (mem_pc_valid && mem_misprediction) || (mem_reg_valid && mem_reg_sfence)

  // Threadlet control side-effects must only occur when this MEM-stage instruction is committed.
  val mem_thread_commit = mem_thread_ctrl_valid &&
    !mem_misprediction &&
    !mem_reg_xcpt &&
    !mem_reg_xcpt_interrupt &&
    !(take_pc_wb && wb_thread === mem_thread)
  
  thread_yield := mem_thread_commit &&
    (mem_ctrl.thread.halt || mem_ctrl.thread.yields || mem_ctrl.thread.pass)

  mem_reg_valid := !ctrl_killx
  mem_reg_replay := !needToClear(ex_thread) && replay_ex
  mem_reg_xcpt := !ctrl_killx && ex_xcpt
  mem_reg_xcpt_interrupt := !needToClear(ex_thread) && ex_reg_xcpt_interrupt

  when (mem_thread_commit && mem_ctrl.thread.test) {
    interrupt_threadlet_mode := true.B
    mem_reg_test := true.B
  } .elsewhen (mem_thread_commit && mem_ctrl.thread.enable) {
    print_enable := true.B
  } .elsewhen (mem_thread_commit && mem_ctrl.thread.disable) {
    print_enable := false.B
    intr_print_active := false.B
    intr_wait_switch := false.B
  }

  coreDmem.dcache_monitor.valid := false.B
  coreDmem.dcache_monitor.bits.set := false.B
  coreDmem.dcache_monitor.bits.clear := false.B
  coreDmem.dcache_monitor.bits.thread := 0.U
  coreDmem.dcache_monitor.bits.slot := 0.U
  coreDmem.dcache_monitor.bits.addr := 0.U

  tm.io.ctrl.valid := mem_thread_commit
  tm.io.ctrl.bits.init := mem_ctrl.thread.init
  tm.io.ctrl.bits.create := mem_ctrl.thread.create
  tm.io.ctrl.bits.halt := mem_ctrl.thread.halt
  tm.io.ctrl.bits.yields := mem_ctrl.thread.yields
  tm.io.ctrl.bits.pass := mem_ctrl.thread.pass
  tm.io.ctrl.bits.create_pc := mem_thread_rs0
  tm.io.ctrl.bits.set_prior := mem_ctrl.thread.set_prior
  tm.io.ctrl.bits.set_slice := mem_ctrl.thread.set_slice
  tm.io.ctrl.bits.set_deadline := mem_ctrl.thread.set_deadline
  tm.io.ctrl.bits.wakeup := mem_ctrl.thread.wakeup
  tm.io.ctrl.bits.syn_print := mem_ctrl.thread.syn_print
  tm.io.ctrl.bits.set_base := mem_ctrl.thread.set_base
  tm.io.ctrl.bits.eret := mem_ctrl.thread.eret
  tm.io.ctrl.bits.dcache_monitor_set := mem_ctrl.thread.dcache_monitor_set
  tm.io.ctrl.bits.dcache_monitor_clear := mem_ctrl.thread.dcache_monitor_clear
  tm.io.ctrl.bits.dcache_monitor_addr := mem_thread_rs0(paddrBits - 1, 0)

  tm.io.ctrl.bits.thread := mem_thread
  tm.io.ctrl.bits.prior := mem_thread_rs1
  tm.io.ctrl.bits.prior_thread := mem_thread_rs0
  tm.io.ctrl.bits.slice := mem_thread_rs1
  tm.io.ctrl.bits.slice_thread := mem_thread_rs0
  tm.io.ctrl.bits.deadline := mem_thread_rs1
  tm.io.ctrl.bits.deadline_thread := mem_thread_rs0
  tm.io.ctrl.bits.wakeup_thread := mem_thread_rs0
  tm.io.ctrl.bits.syn_stage := mem_thread_rs0
  tm.io.ctrl.bits.syn_data := mem_thread_rs1
  tm.io.ctrl.bits.interrupt_base := mem_thread_rs0

  (threadCtxMgr, threadCtxSaver) match {
    case (Some(ctxMgr), Some(saver)) =>
      val base = threadlet_ctx_base.get
      val base_valid = threadlet_ctx_base_valid.get
      when (mem_thread_commit && mem_ctrl.thread.set_ctx_base) {
        base := mem_thread_rs0(paddrBits - 1, 0)
        base_valid := true.B
      }

      ctxMgr.io.tpt_read_req <> tm.io.tpt_read_req
      ctxMgr.io.tpt_read_resp := tm.io.tpt_read_resp
      ctxMgr.io.tpt_ready_chunks := tm.io.tpt_ready_chunks
      ctxMgr.io.slot_meta := tm.io.slot_meta
      ctxMgr.io.current_thread := tm.io.current_thread_out
      ctxMgr.io.rank_change <> tm.io.rank_change
      ctxMgr.io.metadata_pending := tm.io.metadata_pending
      ctxMgr.io.can_start := base_valid && (saver.io.busy_slots === 0.U)
      ctxMgr.io.ctx_write_busy := ctxWriteInFlight
      ctxMgr.io.saver_busy_slots := saver.io.busy_slots
      ctxMgr.io.save_done := saver.io.save_done
      ctxMgr.io.load_done := saver.io.load_done
      ctxMgr.io.slot_drain_busy := ctxSlotDrainBusy
      ctxMgr.io.slot_dirty_set := ctxSlotDirtySet
      ctxThreadKill := ctxMgr.io.kill_thread

      saver.io.base := base
      saver.io.base_valid := base_valid
      saver.io.save_start := ctxMgr.io.save_start
      saver.io.load_start := ctxMgr.io.load_start
      saver.io.load_redirect <> ctxMgr.io.load_redirect
      saver.io.rf_rdata := rf.read(saver.io.rf_raddr)
      tm.io.slot_thread := ctxMgr.io.slot_thread
      tm.io.slot_sched_blocked := ctxMgr.io.slot_sched_blocked
      tm.io.slot_load_request := ctxMgr.io.slot_load_request
      ctxMgr.io.slot_load_commit := tm.io.slot_load_commit
    case _ =>
      tm.io.slot_thread := VecInit(Seq.fill(threadSlotCount)(0.U(threadIdLength.W)))
      tm.io.slot_sched_blocked := 0.U(threadSlotCount.W)
      tm.io.slot_load_request.valid := false.B
      tm.io.slot_load_request.bits := 0.U.asTypeOf(new ThreadContextOp)
      tm.io.tpt_read_req.valid := false.B
      tm.io.tpt_read_req.bits := 0.U.asTypeOf(new ThreadTptReadReq)
      tm.io.rank_change.ready := true.B
  }

  tm.io.interrupt := Mux(interrupt_threadlet_mode,
    wb_reg_xcpt_interrupt && csr.io.interrupt_deleg,
    wb_interrupt_redirect)

  tm.io.dcache_wakeup := coreDmem.dcache_wakeup
  tm.io.dcache_probe := coreDmem.dcache_probe
  tm.io.dcache_self_evict := coreDmem.dcache_self_evict
  tm.io.hartid := io.hartid

  tm.io.mem_info.valid := mem_reg_valid && !mem_misprediction
  tm.io.mem_info.bits.thread := mem_thread
  tm.io.mem_info.bits.pc := mem_npc

  tm.io.mem_req := mem_thread
  // on pipeline flushes, cause mem_npc to hold the sequential npc, which
  // will drive the W-stage npc mux
  when (mem_reg_valid && mem_reg_flush_pipe) {
    mem_reg_sfence := false.B
  }.elsewhen (ex_pc_valid) {
    mem_thread := ex_thread
    mem_slot := ex_slot
    mem_ctrl := ex_ctrl
    mem_thread_ctx_write_slot := ex_thread_ctx_write_slot
    mem_thread_ctx_write_resident := ex_thread_ctx_write_resident
    mem_thread_rs0 := ex_rs(0)
    mem_thread_rs1 := ex_rs(1)
    mem_reg_rvc := ex_reg_rvc
    mem_reg_load := ex_ctrl.mem && isRead(ex_ctrl.mem_cmd)
    mem_reg_store := ex_ctrl.mem && isWrite(ex_ctrl.mem_cmd)
    mem_reg_sfence := ex_sfence
    mem_reg_btb_resp := ex_reg_btb_resp
    mem_reg_flush_pipe := ex_reg_flush_pipe
    mem_reg_slow_bypass := ex_slow_bypass
    mem_reg_wphit := ex_reg_wphit
    mem_reg_set_vconfig := ex_reg_set_vconfig

    mem_reg_cause := ex_cause
    mem_reg_inst := ex_reg_inst
    mem_reg_raw_inst := ex_reg_raw_inst
    mem_reg_mem_size := ex_reg_mem_size
    mem_reg_hls_or_dv := coreDmem.req.bits.dv
    mem_reg_pc := ex_reg_pc
    // IDecode ensured they are 1H
    mem_reg_wdata := Mux(ex_reg_set_vconfig, 
          ex_new_vl.getOrElse(alu.io.out), 
          alu.io.out)
    mem_br_taken := alu.io.cmp_out

    when (ex_ctrl.rxs2 && (ex_ctrl.mem || ex_ctrl.rocc || ex_sfence)) {
      val size = Mux(ex_ctrl.rocc, log2Ceil(xLen/8).U, ex_reg_mem_size)
      mem_reg_rs2 := new StoreGen(size, 0.U, ex_rs(1), coreDataBytes).data
    }
    if (usingVector) { when (ex_reg_set_vconfig) {
      mem_reg_rs2 := ex_new_vconfig.get.asUInt
    } }
    when (ex_ctrl.jalr && csr.io.status.debug) {
      // flush I$ on D-mode JALR to effect uncached fetch without D$ flush
      mem_ctrl.fence_i := true.B
      mem_reg_flush_pipe := true.B
    }
  }

  val mem_breakpoint = (mem_reg_load && bpu.io.xcpt_ld) || (mem_reg_store && bpu.io.xcpt_st)
  val mem_debug_breakpoint = (mem_reg_load && bpu.io.debug_ld) || (mem_reg_store && bpu.io.debug_st)
  val (mem_ldst_xcpt, mem_ldst_cause) = checkExceptions(List(
    (mem_debug_breakpoint, CSR.debugTriggerCause.U),
    (mem_breakpoint,       Causes.breakpoint.U)))

  val (mem_xcpt, mem_cause) = checkExceptions(List(
    (mem_reg_xcpt_interrupt || mem_reg_xcpt, mem_reg_cause),
    (mem_reg_valid && mem_npc_misaligned,    Causes.misaligned_fetch.U),
    (mem_reg_valid && mem_ldst_xcpt,         mem_ldst_cause)))

  val memCoverCauses = (exCoverCauses ++ List(
    (CSR.debugTriggerCause, "DEBUG_TRIGGER"),
    (Causes.breakpoint, "BREAKPOINT"),
    (Causes.misaligned_fetch, "MISALIGNED_FETCH")
  )).distinct
  coverExceptions(mem_xcpt, mem_cause, "MEMORY", memCoverCauses)

  val dcache_kill_mem = mem_reg_valid && mem_ctrl.wxd && coreDmem.replay_next // structural hazard on writeback port
  val fpu_kill_mem = mem_reg_valid && mem_ctrl.fp && io.fpu.nack_mem
  val vec_kill_mem = mem_reg_valid && mem_ctrl.mem && io.vector.map(_.mem.block_mem).getOrElse(false.B)
  val vec_kill_all = mem_reg_valid && io.vector.map(_.mem.block_all).getOrElse(false.B)
  val replay_mem  = dcache_kill_mem || mem_reg_replay || fpu_kill_mem || vec_kill_mem || vec_kill_all
  val killm_common = dcache_kill_mem || (take_pc_wb && wb_thread === mem_thread) || mem_reg_xcpt || !mem_reg_valid || mem_misprediction
  div.io.kill := killm_common && RegNext(div.io.req.fire)
  val ctrl_killm = killm_common || mem_xcpt || fpu_kill_mem || vec_kill_mem

  // writeback stage
  wb_reg_valid := !ctrl_killm
  wb_reg_replay := replay_mem && !(take_pc_wb && wb_thread === mem_thread) && !(mem_misprediction && mem_pc_valid)
  wb_reg_xcpt := mem_xcpt && !(take_pc_wb && wb_thread === mem_thread) && !(mem_misprediction && mem_pc_valid) && !io.vector.map(_.mem.block_all).getOrElse(false.B)
  wb_reg_flush_pipe := !ctrl_killm && mem_reg_flush_pipe
  // By default, assume no interrupt-class trap in WB; overwrite when we latch a valid mem stage.
  wb_reg_xcpt_interrupt := false.B
  when (mem_pc_valid) {
    wb_thread := mem_thread
    wb_slot := mem_slot
    wb_ctrl := mem_ctrl
    wb_thread_ctx_write_slot := mem_thread_ctx_write_slot
    wb_thread_ctx_write_resident := mem_thread_ctx_write_resident
    wb_reg_xcpt_interrupt := mem_reg_xcpt_interrupt
    wb_reg_sfence := mem_reg_sfence
    wb_reg_wdata := Mux(!mem_reg_xcpt && mem_ctrl.fp && mem_ctrl.wxd, io.fpu.toint_data, mem_int_wdata)
    when (mem_ctrl.rocc || mem_reg_sfence || mem_reg_set_vconfig) {
      wb_reg_rs2 := mem_reg_rs2
    }
    wb_reg_cause := mem_cause
    wb_reg_inst := mem_reg_inst
    wb_reg_raw_inst := mem_reg_raw_inst
    wb_reg_mem_size := mem_reg_mem_size
    wb_reg_hls_or_dv := mem_reg_hls_or_dv
    wb_reg_hfence_v := mem_ctrl.mem_cmd === M_HFENCEV
    wb_reg_hfence_g := mem_ctrl.mem_cmd === M_HFENCEG
    wb_reg_pc := mem_reg_pc
    wb_reg_br_taken := mem_br_taken
    wb_reg_wphit := mem_reg_wphit | bpu.io.bpwatch.map { bpw => (bpw.rvalid(0) && mem_reg_load) || (bpw.wvalid(0) && mem_reg_store) }
    wb_reg_set_vconfig := mem_reg_set_vconfig
  }

  when (intr_print_active) {
    when (intr_wait_switch && id_thread === 0.U(threadIdLength.W)) {
      intr_print_active := false.B
      intr_wait_switch := false.B
    } .elsewhen (wb_reg_valid && wb_ctrl.thread.eret) {
      intr_wait_switch := true.B
    }
  }

  val (wb_xcpt, wb_cause) = checkExceptions(List(
    (wb_reg_xcpt,  wb_reg_cause),
    (wb_reg_valid && wb_ctrl.mem && coreDmem.s2_xcpt.pf.st, Causes.store_page_fault.U),
    (wb_reg_valid && wb_ctrl.mem && coreDmem.s2_xcpt.pf.ld, Causes.load_page_fault.U),
    (wb_reg_valid && wb_ctrl.mem && coreDmem.s2_xcpt.gf.st, Causes.store_guest_page_fault.U),
    (wb_reg_valid && wb_ctrl.mem && coreDmem.s2_xcpt.gf.ld, Causes.load_guest_page_fault.U),
    (wb_reg_valid && wb_ctrl.mem && coreDmem.s2_xcpt.ae.st, Causes.store_access.U),
    (wb_reg_valid && wb_ctrl.mem && coreDmem.s2_xcpt.ae.ld, Causes.load_access.U),
    (wb_reg_valid && wb_ctrl.mem && coreDmem.s2_xcpt.ma.st, Causes.misaligned_store.U),
    (wb_reg_valid && wb_ctrl.mem && coreDmem.s2_xcpt.ma.ld, Causes.misaligned_load.U)
  ))

  val wbCoverCauses = List(
    (Causes.misaligned_store, "MISALIGNED_STORE"),
    (Causes.misaligned_load, "MISALIGNED_LOAD"),
    (Causes.store_access, "STORE_ACCESS"),
    (Causes.load_access, "LOAD_ACCESS")
  ) ++ (if(usingVM) List(
    (Causes.store_page_fault, "STORE_PAGE_FAULT"),
    (Causes.load_page_fault, "LOAD_PAGE_FAULT")
  ) else Nil) ++ (if (usingHypervisor) List(
    (Causes.store_guest_page_fault, "STORE_GUEST_PAGE_FAULT"),
    (Causes.load_guest_page_fault, "LOAD_GUEST_PAGE_FAULT"),
  ) else Nil)
  coverExceptions(wb_xcpt, wb_cause, "WRITEBACK", wbCoverCauses)

  val wb_pc_valid = wb_reg_valid || wb_reg_replay || wb_reg_xcpt
  val wb_wxd = wb_reg_valid && wb_ctrl.wxd
  val wb_set_sboard = wb_ctrl.div || wb_dcache_miss || wb_ctrl.rocc || wb_ctrl.vec
  val replay_wb_common = coreDmem.s2_nack || wb_reg_replay
  val replay_wb_rocc = wb_reg_valid && wb_ctrl.rocc && !io.rocc.cmd.ready
  val replay_wb_csr: Bool = wb_reg_valid && csr.io.rw_stall
  val replay_wb_vec = wb_reg_valid && io.vector.map(_.wb.replay).getOrElse(false.B)
  val replay_wb = replay_wb_common || replay_wb_rocc || replay_wb_csr || replay_wb_vec
  take_pc_wb := replay_wb || wb_xcpt || csr.io.eret || wb_reg_flush_pipe

  tm.io.wb_info.valid := take_pc_wb && !(wb_reg_xcpt_interrupt && interrupt_threadlet_mode && csr.io.interrupt_deleg)
  
  tm.io.wb_info.bits.thread := wb_thread
  tm.io.wb_info.bits.pc := Mux(wb_xcpt || csr.io.eret, csr.io.evec,  // exception or [m|s]ret
                           Mux(replay_wb,              wb_reg_pc,    // replay
                                                        mem_npc))    // flush

  // writeback arbitration
  val dmem_resp_xpu = !coreDmem.resp.bits.tag(0).asBool
  val dmem_resp_fpu =  coreDmem.resp.bits.tag(0).asBool
  val dmem_resp_waddr = coreDmem.resp.bits.tag(rfAddrBits, 1)
  val dmem_resp_valid = coreDmem.resp.valid && coreDmem.resp.bits.has_data
  val dmem_resp_replay = dmem_resp_valid && coreDmem.resp.bits.replay

  class LLWB extends Bundle {
    val data = UInt(xLen.W)
    val tag = UInt(rfAddrBits.W)
  }

  val ll_arb = Module(new Arbiter(new LLWB, 3)) // div, rocc, vec
  ll_arb.io.in.foreach(_.valid := false.B)
  ll_arb.io.in.foreach(_.bits := DontCare)
  val ll_wdata = WireInit(ll_arb.io.out.bits.data)
  val ll_waddr = WireInit(ll_arb.io.out.bits.tag)
  val ll_wen = WireInit(ll_arb.io.out.fire)
  ll_arb.io.out.ready := !wb_wxd

  div.io.resp.ready := ll_arb.io.in(0).ready
  ll_arb.io.in(0).valid := div.io.resp.valid
  ll_arb.io.in(0).bits.data := div.io.resp.bits.data
  ll_arb.io.in(0).bits.tag := div.io.resp.bits.tag

  if (usingRoCC) {
    io.rocc.resp.ready := ll_arb.io.in(1).ready
    ll_arb.io.in(1).valid := io.rocc.resp.valid
    ll_arb.io.in(1).bits.data := io.rocc.resp.bits.data
    ll_arb.io.in(1).bits.tag := io.rocc.resp.bits.rd
  } else {
    // tie off RoCC
    io.rocc.resp.ready := false.B
    io.rocc.mem.req.ready := false.B
  }

  io.vector.map { v =>
    v.resp.ready := Mux(v.resp.bits.fp, !(dmem_resp_valid && dmem_resp_fpu), ll_arb.io.in(2).ready)
    ll_arb.io.in(2).valid := v.resp.valid && !v.resp.bits.fp
    ll_arb.io.in(2).bits.data := v.resp.bits.data
    ll_arb.io.in(2).bits.tag := v.resp.bits.rd
  }
  // Dont care mem since not all RoCC need accessing memory
  io.rocc.mem := DontCare

  when (dmem_resp_replay && dmem_resp_xpu) {
    ll_arb.io.out.ready := false.B
    ll_waddr := dmem_resp_waddr
    ll_wen := true.B
  }

  val wb_valid = wb_reg_valid && !replay_wb && !wb_xcpt
  when (wb_valid && wb_ctrl.thread.legal && wb_ctrl.thread.dcache_print_enable && !dcache_print_enable) {
    dcache_print_enable := true.B
  }
  when (wb_valid && wb_ctrl.thread.legal && wb_ctrl.thread.dcache_print_disable && dcache_print_enable) {
    dcache_print_enable := false.B
  }
  val wb_thread_ctx_write_drop = wb_ctrl.thread.legal &&
    wb_ctrl.thread.ctx_write &&
    !wb_thread_ctx_write_resident
  val wb_wen = wb_valid && wb_ctrl.wxd && !wb_thread_ctx_write_drop
  val rf_wen = wb_wen || ll_wen
  val rf_waddr = Mux(ll_wen, ll_waddr, wb_waddr)
  val rf_wdata = Mux(dmem_resp_valid && dmem_resp_xpu, coreDmem.resp.bits.data(xLen-1, 0),
                 Mux(ll_wen, ll_wdata,
                 Mux(wb_ctrl.csr =/= CSR.N, csr.io.rw.rdata,
                 Mux(wb_ctrl.mul, mul.map(_.io.resp.bits.data).getOrElse(wb_reg_wdata),
                 wb_reg_wdata))))
  val ctx_rf_wen = threadCtxSaver.map(_.io.rf_wen).getOrElse(false.B)
  val ctx_rf_waddr = threadCtxSaver.map(_.io.rf_waddr(rfAddrBits - 1, 0)).getOrElse(0.U(rfAddrBits.W))
  val ctx_rf_wdata = threadCtxSaver.map(_.io.rf_wdata).getOrElse(0.U(xLen.W))
  threadCtxSaver.foreach { saver =>
    saver.io.rf_wready := !rf_wen
  }
  ctxSlotDirtySet.valid := rf_wen && !rfAddrIsX0(rf_waddr)
  ctxSlotDirtySet.bits :=
    (if (threadSlotCount == 1) 0.U(threadSlotIdLength.W)
     else rf_waddr(rfAddrBits - 1, lgNXRegs))
  val rf_wen_any = rf_wen || ctx_rf_wen
  val rf_waddr_final = Mux(rf_wen, rf_waddr, ctx_rf_waddr)
  val rf_wdata_final = Mux(rf_wen, rf_wdata, ctx_rf_wdata)
  if (coreParams.threadletAreaDebugAssert) {
    assert(!(rf_wen && ctx_rf_wen), "context loader RF write must be mutually exclusive with pipeline writeback")
  }
  when (rf_wen_any) {
    rf.write(rf_waddr_final, rf_wdata_final)
  }

  if (rocketParams.enableTraceCoreIngress) {
    val trace_ingress = Module(new TraceCoreIngress(traceIngressParams))
    trace_ingress.io.in.valid := wb_valid || wb_xcpt
    trace_ingress.io.in.taken := wb_reg_br_taken
    trace_ingress.io.in.is_branch := wb_ctrl.branch
    trace_ingress.io.in.is_jal := wb_ctrl.jal
    trace_ingress.io.in.is_jalr := wb_ctrl.jalr
    trace_ingress.io.in.insn := wb_reg_inst
    trace_ingress.io.in.pc := wb_reg_pc
    trace_ingress.io.in.is_compressed := !wb_reg_raw_inst(1, 0).andR // 2'b11 is uncompressed, everything else is compressed
    trace_ingress.io.in.interrupt := csr.io.trace(0).interrupt && csr.io.trace(0).exception
    trace_ingress.io.in.exception := !csr.io.trace(0).interrupt && csr.io.trace(0).exception
    trace_ingress.io.in.trap_return := csr.io.trap_return

    io.trace_core_ingress.get.group(0) <> trace_ingress.io.out
    io.trace_core_ingress.get.priv := csr.io.trace(0).priv 
    io.trace_core_ingress.get.tval := csr.io.tval
    io.trace_core_ingress.get.cause := csr.io.cause
    io.trace_core_ingress.get.time := csr.io.time
  }

  // hook up control/status regfile
  csr.io.ungated_clock := clock
  csr.io.decode(0).inst := id_inst(0)
  csr.io.exception := Mux(wb_interrupt_redirect, wb_interrupt_redirect, wb_xcpt)
  csr.io.cause := Mux(wb_interrupt_redirect, wb_interrupt_redirect_cause, wb_cause)
  csr.io.retire := wb_valid
  csr.io.inst(0) := (if (usingCompressed) Cat(Mux(wb_reg_raw_inst(1, 0).andR, wb_reg_inst >> 16, 0.U), wb_reg_raw_inst(15, 0)) else wb_reg_inst)
  csr.io.interrupts := io.interrupts
  csr.io.hartid := io.hartid
  io.fpu.fcsr_rm := csr.io.fcsr_rm
  val vector_fcsr_flags = io.vector.map(_.set_fflags.bits).getOrElse(0.U(5.W))
  val vector_fcsr_flags_valid = io.vector.map(_.set_fflags.valid).getOrElse(false.B)
  csr.io.fcsr_flags.valid := io.fpu.fcsr_flags.valid | vector_fcsr_flags_valid
  csr.io.fcsr_flags.bits := (io.fpu.fcsr_flags.bits & Fill(5, io.fpu.fcsr_flags.valid)) | (vector_fcsr_flags & Fill(5, vector_fcsr_flags_valid))
  io.fpu.time := csr.io.time(31,0)
  io.fpu.hartid := io.hartid
  csr.io.rocc_interrupt := io.rocc.interrupt
  csr.io.pc := wb_reg_pc

  val tval_dmem_addr = !wb_reg_xcpt
  val tval_any_addr = tval_dmem_addr ||
    wb_reg_cause.isOneOf(Causes.breakpoint.U, Causes.fetch_access.U, Causes.fetch_page_fault.U, Causes.fetch_guest_page_fault.U)
  val tval_inst = wb_reg_cause === Causes.illegal_instruction.U
  val tval_valid = wb_xcpt && (tval_any_addr || tval_inst)
  csr.io.gva := wb_xcpt && (tval_any_addr && csr.io.status.v || tval_dmem_addr && wb_reg_hls_or_dv)
  csr.io.tval := Mux(tval_valid, encodeVirtualAddress(wb_reg_wdata, wb_reg_wdata), 0.U)
  val (htval, mhtinst_read_pseudo) = {
    val htval_valid_imem = wb_reg_xcpt && wb_reg_cause === Causes.fetch_guest_page_fault.U
    val htval_imem = Mux(htval_valid_imem, io.imem.gpa.bits, 0.U)
    assert(!htval_valid_imem || io.imem.gpa.valid)

    val htval_valid_dmem = wb_xcpt && tval_dmem_addr && coreDmem.s2_xcpt.gf.asUInt.orR && !coreDmem.s2_xcpt.pf.asUInt.orR
    val htval_dmem = Mux(htval_valid_dmem, coreDmem.s2_gpa, 0.U)

    val htval = (htval_dmem | htval_imem) >> hypervisorExtraAddrBits
    // read pseudoinstruction if a guest-page fault is caused by an implicit memory access for VS-stage address translation
    val mhtinst_read_pseudo = (io.imem.gpa_is_pte && htval_valid_imem) || (coreDmem.s2_gpa_is_pte && htval_valid_dmem)
    (htval, mhtinst_read_pseudo)
  }

  csr.io.vector.foreach { v =>
    v.set_vconfig.valid := wb_reg_set_vconfig && wb_reg_valid
    v.set_vconfig.bits := wb_reg_rs2.asTypeOf(new VConfig)
    v.set_vs_dirty := wb_valid && wb_ctrl.vec
    v.set_vstart.valid := wb_valid && wb_reg_set_vconfig
    v.set_vstart.bits := 0.U
  }

  io.vector.foreach { v =>
    when (v.wb.retire || v.wb.xcpt || wb_ctrl.vec) {
      csr.io.pc := v.wb.pc
      csr.io.retire := v.wb.retire
      csr.io.inst(0) := v.wb.inst
      when (v.wb.xcpt && !wb_reg_xcpt) {
        wb_xcpt := true.B
        wb_cause := v.wb.cause
        csr.io.tval := v.wb.tval
      }
    }
    v.wb.store_pending := coreDmem.store_pending
    v.wb.vxrm := csr.io.vector.get.vxrm
    v.wb.frm := csr.io.fcsr_rm
    csr.io.vector.get.set_vxsat := v.set_vxsat
    when (v.set_vconfig.valid) {
      csr.io.vector.get.set_vconfig.valid := true.B
      csr.io.vector.get.set_vconfig.bits := v.set_vconfig.bits
    }
    when (v.set_vstart.valid) {
      csr.io.vector.get.set_vstart.valid := true.B
      csr.io.vector.get.set_vstart.bits := v.set_vstart.bits
    }
  }

  csr.io.htval := htval
  csr.io.mhtinst_read_pseudo := mhtinst_read_pseudo
  io.ptw.ptbr := csr.io.ptbr
  io.ptw.hgatp := csr.io.hgatp
  io.ptw.vsatp := csr.io.vsatp
  (io.ptw.customCSRs.csrs zip csr.io.customCSRs).map { case (lhs, rhs) => lhs <> rhs }
  io.ptw.status := csr.io.status
  io.ptw.hstatus := csr.io.hstatus
  io.ptw.gstatus := csr.io.gstatus
  io.ptw.pmp := csr.io.pmp
  csr.io.rw.addr := wb_reg_inst(31,20)
  csr.io.rw.cmd := CSR.maskCmd(wb_reg_valid, wb_ctrl.csr)
  csr.io.rw.wdata := wb_reg_wdata


  io.rocc.csrs <> csr.io.roccCSRs
  io.trace.time := csr.io.time
  io.trace.insns := csr.io.trace
  if (rocketParams.debugROB.isDefined) {
    val sz = rocketParams.debugROB.get.size
    if (sz < 1) { // use unsynthesizable ROB
      val csr_trace_with_wdata = WireInit(csr.io.trace(0))
      csr_trace_with_wdata.wdata.get := rf_wdata
      val should_wb = WireInit((wb_ctrl.wfd || (wb_ctrl.wxd && !rfAddrIsX0(wb_waddr))) && !csr.io.trace(0).exception)
      val has_wb = WireInit(wb_ctrl.wxd && wb_wen && !wb_set_sboard)
      val wb_addr = WireInit(rfAddrReg(wb_waddr) + Mux(wb_ctrl.wfd, 32.U, 0.U))

      io.vector.foreach { v => when (v.wb.retire) {
        should_wb := v.wb.rob_should_wb
        has_wb := false.B
        wb_addr := Cat(v.wb.rob_should_wb_fp, csr_trace_with_wdata.insn(11,7))
      }}

      DebugROB.pushTrace(clock, reset,
        io.hartid, csr_trace_with_wdata,
        should_wb, has_wb, wb_addr)

      io.trace.insns(0) := DebugROB.popTrace(clock, reset, io.hartid)

      DebugROB.pushWb(clock, reset, io.hartid, ll_wen, rfAddrReg(rf_waddr), rf_wdata)
    } else { // synthesizable ROB (no FPRs)
      require(!usingVector, "Synthesizable ROB does not support vector implementations")
      val csr_trace_with_wdata = WireInit(csr.io.trace(0))
      csr_trace_with_wdata.wdata.get := rf_wdata

      val debug_rob = Module(new HardDebugROB(sz, 32))
      debug_rob.io.i_insn := csr_trace_with_wdata
      debug_rob.io.should_wb := (wb_ctrl.wfd || (wb_ctrl.wxd && !rfAddrIsX0(wb_waddr))) &&
                                !csr.io.trace(0).exception
      debug_rob.io.has_wb := wb_ctrl.wxd && wb_wen && !wb_set_sboard
      debug_rob.io.tag    := rfAddrReg(wb_waddr) + Mux(wb_ctrl.wfd, 32.U, 0.U)

      debug_rob.io.wb_val  := ll_wen
      debug_rob.io.wb_tag  := rfAddrReg(rf_waddr)
      debug_rob.io.wb_data := rf_wdata

      io.trace.insns(0) := debug_rob.io.o_insn
    }
  } else {
    io.trace.insns := csr.io.trace
  }
  for (((iobpw, wphit), bp) <- io.bpwatch zip wb_reg_wphit zip csr.io.bp) {
    iobpw.valid(0) := wphit
    iobpw.action := bp.control.action
    // tie off bpwatch valids
    iobpw.rvalid.foreach(_ := false.B)
    iobpw.wvalid.foreach(_ := false.B)
    iobpw.ivalid.foreach(_ := false.B)
  }

  val hazard_targets = Seq((id_ctrl.rxs1 && id_raddr1(lgNXRegs-1, 0) =/= 0.U, id_raddr1),
                           (id_ctrl.rxs2 && id_raddr2(lgNXRegs-1, 0) =/= 0.U, id_raddr2),
                           (id_ctrl.wxd  && id_waddr(lgNXRegs-1, 0)  =/= 0.U, id_waddr))
  val fp_hazard_targets = Seq((io.fpu.dec.ren1, id_raddr1),
                              (io.fpu.dec.ren2, id_raddr2),
                              (io.fpu.dec.ren3, id_raddr3),
                              (io.fpu.dec.wen, id_waddr))

  val sboard = new Scoreboard(32 * threadSlotCount, true)
  sboard.clear(ll_wen, ll_waddr)
  def id_sboard_clear_bypass(r: UInt) = {
    // ll_waddr arrives late when D$ has ECC, so reshuffle the hazard check
    if (!tileParams.dcache.get.dataECC.isDefined) ll_wen && ll_waddr === r
    else div.io.resp.fire && div.io.resp.bits.tag === r || dmem_resp_replay && dmem_resp_xpu && dmem_resp_waddr === r
  }
  val id_sboard_hazard = checkHazards(hazard_targets, rd => sboard.read(rd) && !id_sboard_clear_bypass(rd))
  sboard.set(wb_set_sboard && wb_wen, wb_waddr)

  val slot_drain_busy = VecInit((0 until threadSlotCount).map { s =>
    val slot = s.U(threadSlotIdLength.W)
    val id_ctx_write_target = id_rs(0)(threadIdLength - 1, 0)
    val id_ctx_write_resident = threadResident(id_ctx_write_target)
    val id_ctx_write_slot = threadToSlot(id_ctx_write_target)
    val id_busy = id_valid && threadResident(id_thread) && (threadToSlot(id_thread) === slot)
    val ex_busy = ex_pc_valid && (ex_slot === slot)
    val mem_busy = mem_pc_valid && (mem_slot === slot)
    val wb_busy = wb_reg_valid && (wb_slot === slot)
    val id_ctx_write_busy = id_valid && id_ctrl.thread.legal && id_ctrl.thread.ctx_write &&
      id_ctx_write_resident && (id_ctx_write_slot === slot)
    val ex_ctx_write_busy = ex_reg_valid && ex_ctrl.thread.legal && ex_ctrl.thread.ctx_write &&
      ex_thread_ctx_write_resident && (ex_thread_ctx_write_slot === slot)
    val mem_ctx_write_busy = mem_reg_valid && mem_ctrl.thread.legal && mem_ctrl.thread.ctx_write &&
      mem_thread_ctx_write_resident && (mem_thread_ctx_write_slot === slot)
    val wb_ctx_write_busy = wb_reg_valid && wb_ctrl.thread.legal && wb_ctrl.thread.ctx_write &&
      wb_thread_ctx_write_resident && (wb_thread_ctx_write_slot === slot)
    val long_busy = (0 until 32).map { r =>
      sboard.read(rfAddr(slot, r.U(lgNXRegs.W)))
    }.reduce(_ || _)
    id_busy || ex_busy || mem_busy || wb_busy ||
      id_ctx_write_busy || ex_ctx_write_busy || mem_ctx_write_busy || wb_ctx_write_busy ||
      long_busy
  })
  ctxSlotDrainBusy := slot_drain_busy.asUInt

  // stall for RAW/WAW hazards on CSRs, loads, AMOs, and mul/div in execute stage.
  val ex_cannot_bypass = ex_ctrl.csr =/= CSR.N || ex_ctrl.jalr || ex_ctrl.mem || ex_ctrl.mul || ex_ctrl.div || ex_ctrl.fp || ex_ctrl.rocc || ex_ctrl.vec
  val data_hazard_ex = ex_ctrl.wxd && !ex_thread_ctx_write_drop && checkHazards(hazard_targets, _ === ex_waddr)
  val fp_data_hazard_ex = id_ctrl.fp && ex_ctrl.wfd && checkHazards(fp_hazard_targets, _ === ex_waddr)
  val id_ex_hazard = ex_reg_valid && (data_hazard_ex && ex_cannot_bypass || fp_data_hazard_ex)

  // stall for RAW/WAW hazards on CSRs, LB/LH, and mul/div in memory stage.
  val mem_mem_cmd_bh =
    if (fastLoadWord) (!fastLoadByte).B && mem_reg_slow_bypass
    else true.B
  val mem_cannot_bypass = mem_ctrl.csr =/= CSR.N || mem_ctrl.mem && mem_mem_cmd_bh || mem_ctrl.mul || mem_ctrl.div || mem_ctrl.fp || mem_ctrl.rocc || mem_ctrl.vec
  val data_hazard_mem = mem_ctrl.wxd && !mem_thread_ctx_write_drop && checkHazards(hazard_targets, _ === mem_waddr)
  val fp_data_hazard_mem = id_ctrl.fp && mem_ctrl.wfd && checkHazards(fp_hazard_targets, _ === mem_waddr)
  val id_mem_hazard = mem_reg_valid && (data_hazard_mem && mem_cannot_bypass || fp_data_hazard_mem)
  id_load_use := mem_reg_valid && data_hazard_mem && mem_ctrl.mem
  val id_vconfig_hazard = id_ctrl.vec && (
    (ex_reg_valid && ex_reg_set_vconfig) ||
    (mem_reg_valid && mem_reg_set_vconfig) ||
    (wb_reg_valid && wb_reg_set_vconfig))

  // stall for RAW/WAW hazards on load/AMO misses and mul/div in writeback.
  val data_hazard_wb = wb_ctrl.wxd && !wb_thread_ctx_write_drop && checkHazards(hazard_targets, _ === wb_waddr)
  val fp_data_hazard_wb = id_ctrl.fp && wb_ctrl.wfd && checkHazards(fp_hazard_targets, _ === wb_waddr)
  val id_wb_hazard = wb_reg_valid && (data_hazard_wb && wb_set_sboard || fp_data_hazard_wb)

  val id_stall_fpu = if (usingFPU) {
    val fp_sboard = new Scoreboard(32)
    fp_sboard.set(((wb_dcache_miss || wb_ctrl.vec) && wb_ctrl.wfd || io.fpu.sboard_set) && wb_valid, wb_waddr)
    val v_ll = io.vector.map(v => v.resp.fire && v.resp.bits.fp).getOrElse(false.B)
    fp_sboard.clear((dmem_resp_replay && dmem_resp_fpu) || v_ll, io.fpu.ll_resp_tag)
    fp_sboard.clear(io.fpu.sboard_clr, io.fpu.sboard_clra)

    checkHazards(fp_hazard_targets, fp_sboard.read _)
  } else false.B

  val dcache_blocked = {
    // speculate that a blocked D$ will unblock the cycle after a Grant
    val blocked = Reg(Bool())
    blocked := !coreDmem.req.ready && coreDmem.clock_enabled && !coreDmem.perf.grant && (blocked || coreDmem.req.valid || coreDmem.s2_nack)
    blocked && !coreDmem.perf.grant
  }
  val rocc_blocked = Reg(Bool())
  rocc_blocked := !wb_xcpt && !io.rocc.cmd.ready && (io.rocc.cmd.valid || rocc_blocked)

  val ctrl_stalld =
    id_ex_hazard || id_mem_hazard || id_wb_hazard || id_sboard_hazard ||
    id_vconfig_hazard ||
    csr.io.singleStep && (ex_reg_valid || mem_reg_valid || wb_reg_valid) ||
    id_csr_en && csr.io.decode(0).fp_csr && !io.fpu.fcsr_rdy ||
    id_csr_en && csr.io.decode(0).vector_csr && id_vec_busy ||
    id_ctrl.fp && id_stall_fpu ||
    id_ctrl.mem && dcache_blocked || // reduce activity during D$ misses
    id_ctrl.rocc && rocc_blocked || // reduce activity while RoCC is busy
    id_ctrl.div && (!(div.io.req.ready || (div.io.resp.valid && !wb_wxd)) || div.io.req.valid) || // reduce odds of replay
    !clock_en ||
    id_do_fence ||
    csr.io.csr_stall ||
    id_reg_pause ||
    io.traceStall
  ctrl_killd := !id_valid || ibuf.io.inst(0).bits.replay || needToClear(id_thread) || ctrl_stalld || interrupt_to_this

  tm.io.new_thread_issue_req.ready := io.imem.new_thread_issue_req.ready
  io.imem.new_thread_issue_req.valid := tm.io.new_thread_issue_req.valid
  io.imem.new_thread_issue_req.bits.pc := tm.io.new_thread_issue_req.bits.pc
  io.imem.new_thread_issue_req.bits.thread := tm.io.new_thread_issue_req.bits.thread
  io.imem.new_thread_issue_req.bits.speculative := !take_pc_wb

  io.imem.kill.valid := take_pc || thread_yield || ctxThreadKill.valid
  io.imem.kill.bits := Mux(ctxThreadKill.valid, ctxThreadKill.bits,
    Mux(take_pc_wb, wb_thread, mem_thread))

  tm.io.xcpt.valid := take_pc
  tm.io.xcpt.bits.thread := Mux(take_pc_wb, wb_thread, mem_thread)
  tm.io.xcpt.bits.wb_xcpt := wb_xcpt
  tm.io.xcpt.bits.eret := csr.io.eret

  io.imem.req.valid := (if (useMultithreading) false.B else take_pc)

  io.imem.req.bits.speculative := !take_pc_wb
  io.imem.req.bits.pc :=
    Mux(wb_xcpt || csr.io.eret, csr.io.evec,
    Mux(replay_wb,              wb_reg_pc,
                                mem_npc))

  io.imem.req.bits.thread := Mux(wb_xcpt || csr.io.eret, wb_thread,
                             Mux(replay_wb,              wb_thread,
                                                         mem_thread))

// TODD(qxh): How does threadlet flush/fence?
  io.imem.flush_icache := wb_reg_valid && wb_ctrl.fence_i && !coreDmem.s2_nack
  io.imem.might_request := {
    imem_might_request_reg := ex_pc_valid || mem_pc_valid || io.ptw.customCSRs.disableICacheClockGate || io.vector.map(_.trap_check_busy).getOrElse(false.B)
    imem_might_request_reg
  }
  io.imem.progress := RegNext(wb_reg_valid && !replay_wb_common)
  io.imem.sfence.valid := wb_reg_valid && wb_reg_sfence
  io.imem.sfence.bits.rs1 := wb_reg_mem_size(0)
  io.imem.sfence.bits.rs2 := wb_reg_mem_size(1)
  io.imem.sfence.bits.addr := wb_reg_wdata
  io.imem.sfence.bits.asid := wb_reg_rs2
  io.imem.sfence.bits.hv := wb_reg_hfence_v
  io.imem.sfence.bits.hg := wb_reg_hfence_g
  io.ptw.sfence := io.imem.sfence

  ibuf.io.inst(0).ready := !ctrl_stalld

  io.imem.btb_update.valid := mem_reg_valid && !take_pc_wb && mem_wrong_npc && (!mem_cfi || mem_cfi_taken)
  io.imem.btb_update.bits.isValid := mem_cfi
  io.imem.btb_update.bits.cfiType :=
    Mux((mem_ctrl.jal || mem_ctrl.jalr) && mem_waddr(0), CFIType.call,
    Mux(mem_ctrl.jalr && (mem_reg_inst(19,15) & regAddrMask.U) === BitPat("b00?01"), CFIType.ret,
    Mux(mem_ctrl.jal || mem_ctrl.jalr, CFIType.jump,
    CFIType.branch)))
  io.imem.btb_update.bits.target := io.imem.req.bits.pc
  io.imem.btb_update.bits.br_pc := (if (usingCompressed) mem_reg_pc + Mux(mem_reg_rvc, 0.U, 2.U) else mem_reg_pc)
  io.imem.btb_update.bits.pc := ~(~io.imem.btb_update.bits.br_pc | (coreInstBytes*fetchWidth-1).U)
  io.imem.btb_update.bits.prediction := mem_reg_btb_resp
  io.imem.btb_update.bits.taken := DontCare

  io.imem.bht_update.valid := mem_reg_valid && !take_pc_wb
  io.imem.bht_update.bits.pc := io.imem.btb_update.bits.pc
  io.imem.bht_update.bits.taken := mem_br_taken
  io.imem.bht_update.bits.mispredict := mem_wrong_npc
  io.imem.bht_update.bits.branch := mem_ctrl.branch
  io.imem.bht_update.bits.prediction := mem_reg_btb_resp.bht

  // Connect RAS in Frontend
  io.imem.ras_update := DontCare

  io.fpu.valid := !ctrl_killd && id_ctrl.fp
  io.fpu.killx := ctrl_killx
  io.fpu.killm := killm_common || vec_kill_mem
  io.fpu.inst := id_inst(0)
  io.fpu.fromint_data := ex_rs(0)
  io.fpu.ll_resp_val := dmem_resp_valid && dmem_resp_fpu
  io.fpu.ll_resp_data := (if (minFLen == 32) coreDmem.resp.bits.data_word_bypass else coreDmem.resp.bits.data)
  io.fpu.ll_resp_type := coreDmem.resp.bits.size
  io.fpu.ll_resp_tag := dmem_resp_waddr(4, 0)
  io.fpu.keep_clock_enabled := io.ptw.customCSRs.disableCoreClockGate

  io.fpu.v_sew := csr.io.vector.map(_.vconfig.vtype.vsew).getOrElse(0.U)

  io.vector.map { v =>
    when (!(dmem_resp_valid && dmem_resp_fpu)) {
      io.fpu.ll_resp_val := v.resp.valid && v.resp.bits.fp
      io.fpu.ll_resp_data := v.resp.bits.data
      io.fpu.ll_resp_type := v.resp.bits.size
      io.fpu.ll_resp_tag := v.resp.bits.rd
    }
  }

  io.vector.foreach { v =>
    v.ex.valid := ex_reg_valid && (ex_ctrl.vec || rocketParams.vector.get.issueVConfig.B && ex_reg_set_vconfig) && !ctrl_killx
    v.ex.inst := ex_reg_inst
    v.ex.vconfig := csr.io.vector.get.vconfig
    v.ex.vstart := Mux(mem_reg_valid && mem_ctrl.vec || wb_reg_valid && wb_ctrl.vec, 0.U, csr.io.vector.get.vstart)
    v.ex.rs1 := ex_rs(0)
    v.ex.rs2 := ex_rs(1)
    v.ex.pc := ex_reg_pc
    v.mem.frs1 := io.fpu.store_data
    v.killm := killm_common || fpu_kill_mem
    v.status := csr.io.status
  }


  coreDmem.req.valid     := ex_reg_valid && ex_ctrl.mem
  val ex_dcache_tag = Cat(ex_waddr, ex_ctrl.fp)
  require(coreParams.dcacheReqTagBits >= ex_dcache_tag.getWidth)
  coreDmem.req.bits.tag  := ex_dcache_tag
  coreDmem.req.bits.cmd  := ex_ctrl.mem_cmd
  coreDmem.req.bits.size := ex_reg_mem_size
  coreDmem.req.bits.signed := !Mux(ex_reg_hls, ex_reg_inst(20), ex_reg_inst(14))
  coreDmem.req.bits.phys := false.B
  coreDmem.req.bits.addr := encodeVirtualAddress(ex_rs(0), alu.io.adder_out)
  coreDmem.req.bits.idx.foreach(_ := coreDmem.req.bits.addr)
  coreDmem.req.bits.dprv := Mux(ex_reg_hls, csr.io.hstatus.spvp, csr.io.status.dprv)
  coreDmem.req.bits.dv := ex_reg_hls || csr.io.status.dv
  coreDmem.req.bits.no_resp := !isRead(ex_ctrl.mem_cmd) || (!ex_ctrl.fp && rfAddrIsX0(ex_waddr))
  coreDmem.req.bits.no_alloc := DontCare
  coreDmem.req.bits.no_xcpt := DontCare
  coreDmem.req.bits.data := DontCare
  coreDmem.req.bits.mask := DontCare

  coreDmem.s1_data.data := (if (fLen == 0) mem_reg_rs2 else Mux(mem_ctrl.fp, Fill(coreDataBits / fLen, io.fpu.store_data), mem_reg_rs2))
  coreDmem.s1_data.mask := DontCare

  coreDmem.s1_kill := killm_common || mem_ldst_xcpt || fpu_kill_mem || vec_kill_mem
  coreDmem.s2_kill := false.B
  // don't let D$ go to sleep if we're probably going to use it soon
  coreDmem.keep_clock_enabled := id_valid && id_ctrl.mem && !csr.io.csr_stall
  coreDmem.dcache_print_enable := dcache_print_enable

  threadCtxDmemArb match {
    case Some(arb) =>
      arb.io.core <> coreDmem
      arb.io.saver <> threadCtxSaver.get.io.cache
      io.dmem <> arb.io.mem
    case None =>
      io.dmem <> coreDmem
  }

  io.rocc.cmd.valid := wb_reg_valid && wb_ctrl.rocc && !replay_wb_common
  io.rocc.exception := wb_xcpt && csr.io.status.xs.orR
  io.rocc.cmd.bits.status := csr.io.status
  io.rocc.cmd.bits.inst := wb_reg_inst.asTypeOf(new RoCCInstruction())
  io.rocc.cmd.bits.rs1 := wb_reg_wdata
  io.rocc.cmd.bits.rs2 := wb_reg_rs2

  // gate the clock
  val unpause = csr.io.time(rocketParams.lgPauseCycles-1, 0) === 0.U || csr.io.inhibit_cycle || coreDmem.perf.release || take_pc
  when (unpause) { id_reg_pause := false.B }
  io.cease := csr.io.status.cease && !clock_en_reg
  io.wfi := csr.io.status.wfi
  if (rocketParams.clockGate) {
    long_latency_stall := csr.io.csr_stall || coreDmem.perf.blocked || id_reg_pause && !unpause
    clock_en := clock_en_reg || ex_pc_valid || (!long_latency_stall && io.imem.resp.valid)
    clock_en_reg :=
      ex_pc_valid || mem_pc_valid || wb_pc_valid || // instruction in flight
      io.ptw.customCSRs.disableCoreClockGate || // chicken bit
      !div.io.req.ready || // mul/div in flight
      usingFPU.B && !io.fpu.fcsr_rdy || // long-latency FPU in flight
      coreDmem.replay_next || // long-latency load replaying
      id_rocc_busy || // RoCC command in flight
      (!long_latency_stall && (ibuf.io.inst(0).valid || io.imem.resp.valid)) // instruction pending

    assert(!(ex_pc_valid || mem_pc_valid || wb_pc_valid) || clock_en)
  }

  // evaluate performance counters
  val icache_blocked = !(io.imem.resp.valid || RegNext(io.imem.resp.valid))
  csr.io.counters foreach { c => c.inc := RegNext(perfEvents.evaluate(c.eventSel)) }

  val coreMonitorBundle = Wire(new CoreMonitorBundle(xLen, fLen))

  coreMonitorBundle.clock := clock
  coreMonitorBundle.reset := reset
  coreMonitorBundle.hartid := io.hartid
  coreMonitorBundle.timer := csr.io.time(31,0)
  coreMonitorBundle.valid := csr.io.trace(0).valid && !csr.io.trace(0).exception
  coreMonitorBundle.pc := csr.io.trace(0).iaddr(vaddrBitsExtended-1, 0).sextTo(xLen)
  coreMonitorBundle.wrenx := wb_wen && !wb_set_sboard
  coreMonitorBundle.wrenf := false.B
  coreMonitorBundle.wrdst := rfAddrReg(wb_waddr)
  coreMonitorBundle.wrdata := rf_wdata
  coreMonitorBundle.rd0src := wb_reg_inst(19,15)
  coreMonitorBundle.rd0val := RegNext(RegNext(ex_rs(0)))
  coreMonitorBundle.rd1src := wb_reg_inst(24,20)
  coreMonitorBundle.rd1val := RegNext(RegNext(ex_rs(1)))
  coreMonitorBundle.inst := csr.io.trace(0).insn
  coreMonitorBundle.excpt := csr.io.trace(0).exception
  coreMonitorBundle.priv_mode := csr.io.trace(0).priv

  io.nic_tester.valid := csr.io.trace(0).valid && !csr.io.trace(0).exception

  if (enableCommitLog) {
    val t = csr.io.trace(0)
      val rd = rfAddrReg(wb_waddr)
    val wfd = wb_ctrl.wfd
    val wxd = wb_ctrl.wxd
    val has_data = wb_wen && !wb_set_sboard

    when (t.valid && !t.exception) {
      when (wfd) {
        printf ("%d 0x%x (0x%x) f%d p%d 0xXXXXXXXXXXXXXXXX\n", t.priv, t.iaddr, t.insn, rd, rd+32.U)
      }
      .elsewhen (wxd && rd =/= 0.U && has_data) {
        printf ("%d 0x%x (0x%x) x%d 0x%x\n", t.priv, t.iaddr, t.insn, rd, rf_wdata)
      }
      .elsewhen (wxd && rd =/= 0.U && !has_data) {
        printf ("%d 0x%x (0x%x) x%d p%d 0xXXXXXXXXXXXXXXXX\n", t.priv, t.iaddr, t.insn, rd, rd)
      }
      .otherwise {
        printf ("%d 0x%x (0x%x)\n", t.priv, t.iaddr, t.insn)
      }
    }

    when (ll_wen && !rfAddrIsX0(rf_waddr)) {
      printf ("x%d p%d 0x%x\n", rfAddrReg(rf_waddr), rf_waddr, rf_wdata)
    }
  }
  else {
    when (csr.io.trace(0).valid) {
      printf("C%d: %d [%d] pc=[%x] W[r%d=%x][%d] R[r%d=%x] R[r%d=%x] inst=[%x] DASM(%x)\n",
         io.hartid, coreMonitorBundle.timer, coreMonitorBundle.valid,
         coreMonitorBundle.pc,
         Mux(wb_ctrl.wxd || wb_ctrl.wfd, coreMonitorBundle.wrdst, 0.U),
         Mux(coreMonitorBundle.wrenx, coreMonitorBundle.wrdata, 0.U),
         coreMonitorBundle.wrenx,
         Mux(wb_ctrl.rxs1 || wb_ctrl.rfs1, coreMonitorBundle.rd0src, 0.U),
         Mux(wb_ctrl.rxs1 || wb_ctrl.rfs1, coreMonitorBundle.rd0val, 0.U),
         Mux(wb_ctrl.rxs2 || wb_ctrl.rfs2, coreMonitorBundle.rd1src, 0.U),
         Mux(wb_ctrl.rxs2 || wb_ctrl.rfs2, coreMonitorBundle.rd1val, 0.U),
         coreMonitorBundle.inst, coreMonitorBundle.inst)
    }
  }


  // CoreMonitorBundle for late latency writes
  val xrfWriteBundle = Wire(new CoreMonitorBundle(xLen, fLen))

  xrfWriteBundle.clock := clock
  xrfWriteBundle.reset := reset
  xrfWriteBundle.hartid := io.hartid
  xrfWriteBundle.timer := csr.io.time(31,0)
  xrfWriteBundle.valid := false.B
  xrfWriteBundle.pc := 0.U
  xrfWriteBundle.wrdst := rfAddrReg(rf_waddr)
  xrfWriteBundle.wrenx := rf_wen && !(csr.io.trace(0).valid && wb_wen && (wb_waddr === rf_waddr))
  xrfWriteBundle.wrenf := false.B
  xrfWriteBundle.wrdata := rf_wdata
  xrfWriteBundle.rd0src := 0.U
  xrfWriteBundle.rd0val := 0.U
  xrfWriteBundle.rd1src := 0.U
  xrfWriteBundle.rd1val := 0.U
  xrfWriteBundle.inst := 0.U
  xrfWriteBundle.excpt := false.B
  xrfWriteBundle.priv_mode := csr.io.trace(0).priv

  if (rocketParams.haveSimTimeout) PlusArg.timeout(
    name = "max_core_cycles",
    docstring = "Kill the emulation after INT rdtime cycles. Off if 0."
  )(csr.io.time)

  } // leaving gated-clock domain
  val rocketImpl = withClock (gated_clock) { new RocketImpl }

  def checkExceptions(x: Seq[(Bool, UInt)]) =
    (WireInit(x.map(_._1).reduce(_||_)), WireInit(PriorityMux(x)))

  def coverExceptions(exceptionValid: Bool, cause: UInt, labelPrefix: String, coverCausesLabels: Seq[(Int, String)]): Unit = {
    for ((coverCause, label) <- coverCausesLabels) {
      property.cover(exceptionValid && (cause === coverCause.U), s"${labelPrefix}_${label}")
    }
  }

  def checkHazards(targets: Seq[(Bool, UInt)], cond: UInt => Bool) =
    targets.map(h => h._1 && cond(h._2)).reduce(_||_)

  def encodeVirtualAddress(a0: UInt, ea: UInt) = if (vaddrBitsExtended == vaddrBits) ea else {
    // efficient means to compress 64-bit VA into vaddrBits+1 bits
    // (VA is bad if VA(vaddrBits) != VA(vaddrBits-1))
    val b = vaddrBitsExtended-1
    val a = (a0 >> b).asSInt
    val msb = Mux(a === 0.S || a === -1.S, ea(b), !ea(b-1))
    Cat(msb, ea(b-1, 0))
  }

  class Scoreboard(n: Int, zero: Boolean = false)
  {
    def set(en: Bool, addr: UInt): Unit = update(en, _next | mask(en, addr))
    def clear(en: Bool, addr: UInt): Unit = update(en, _next & ~mask(en, addr))
    def read(addr: UInt): Bool = r(addr)
    def readBypassed(addr: UInt): Bool = _next(addr)

    private val _r = RegInit(0.U(n.W))
    private val r = if (zero) (_r >> 1 << 1) else _r
    private var _next = r
    private var ens = false.B
    private def mask(en: Bool, addr: UInt) = Mux(en, 1.U << addr, 0.U)
    private def update(en: Bool, update: UInt) = {
      _next = update
      ens = ens || en
      when (ens) { _r := _next }
    }
  }
}

class RegFile(n: Int, w: Int, zero: Boolean = false, threadSlotCount: Int = 1) {
  val rf = Mem(n * threadSlotCount, UInt(w.W))
  private def access(addr: UInt) = 
    rf(addr(log2Up(n) - 1 + log2Up(threadSlotCount),0))

  private val reads = ArrayBuffer[(UInt,UInt)]()
  private var canRead = true

  def read(addr: UInt) = {
    require(canRead)
    reads += addr -> Wire(UInt())
    reads.last._2 := Mux(zero.B && addr(log2Up(n)-1, 0) === 0.U, 0.U, access(addr))
    reads.last._2
  }

  def write(addr: UInt, data: UInt) = {
    canRead = false
    when (addr(log2Up(n) - 1, 0) =/= 0.U) {
      access(addr) := data
      for ((raddr, rdata) <- reads)
        when (addr === raddr) { rdata := data }
    }
  }
}

object ImmGen {
  def apply(sel: UInt, inst: UInt) = {
    val sign = Mux(sel === IMM_Z, 0.S, inst(31).asSInt)
    val b30_20 = Mux(sel === IMM_U, inst(30,20).asSInt, sign)
    val b19_12 = Mux(sel =/= IMM_U && sel =/= IMM_UJ, sign, inst(19,12).asSInt)
    val b11 = Mux(sel === IMM_U || sel === IMM_Z, 0.S,
              Mux(sel === IMM_UJ, inst(20).asSInt,
              Mux(sel === IMM_SB, inst(7).asSInt, sign)))
    val b10_5 = Mux(sel === IMM_U || sel === IMM_Z, 0.U, inst(30,25))
    val b4_1 = Mux(sel === IMM_U, 0.U,
               Mux(sel === IMM_S || sel === IMM_SB, inst(11,8),
               Mux(sel === IMM_Z, inst(19,16), inst(24,21))))
    val b0 = Mux(sel === IMM_S, inst(7),
             Mux(sel === IMM_I, inst(20),
             Mux(sel === IMM_Z, inst(15), 0.U)))

    Cat(sign, b30_20, b19_12, b11, b10_5, b4_1, b0).asSInt
  }
}
