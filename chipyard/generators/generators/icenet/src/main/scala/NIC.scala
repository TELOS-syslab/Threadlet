package icenet

import chisel3._
import chisel3.util._
import chisel3.reflect.DataMirror
import freechips.rocketchip.subsystem.{BaseSubsystem, TLBusWrapperLocation, PBUS, FBUS, InstantiatesHierarchicalElements, HasTileNotificationSinks}
import org.chipsalliance.cde.config.{Field, Parameters}
import freechips.rocketchip.diplomacy._
import freechips.rocketchip.prci._
import freechips.rocketchip.regmapper._
import freechips.rocketchip.interrupts._
import freechips.rocketchip.tilelink._
import freechips.rocketchip.util._
import freechips.rocketchip.tile._
import IceNetConsts._

// This is copied from testchipip to avoid dependencies
class ClockedIO[T <: Data](private val gen: T) extends Bundle {
  val clock = Output(Clock())
  val bits = DataMirror.internal.chiselTypeClone[T](gen)
}

/**
 * @inBufFlits How many flits in the input buffer(s)
 * @outBufFlits Number of flits in the output buffer
 * @nMemXacts Maximum number of transactions that the send/receive path can send to memory
 * @maxAcquireBytes Cache block size
 * @ctrlQueueDepth Depth of the MMIO control queues
 * @usePauser Hardware support for Ethernet pause frames
 * @checksumOffload TCP checksum offload engine
 * @packetMaxBytes Maximum number of bytes in a packet (header size + MTU)
 */
case class NICConfig(
  inBufFlits: Int  = 2 * ETH_STANDARD_MAX_BYTES / NET_IF_BYTES,
  outBufFlits: Int = 2 * ETH_STANDARD_MAX_BYTES / NET_IF_BYTES,
  nMemXacts: Int = 8,
  maxAcquireBytes: Int = 64,
  ctrlQueueDepth: Int = 10,
  usePauser: Boolean = false,
  checksumOffload: Boolean = false,
  packetMaxBytes: Int = ETH_STANDARD_MAX_BYTES,
  nMaxCores: Int = 16)

case class NICAttachParams(
  masterWhere: TLBusWrapperLocation = FBUS,
  slaveWhere: TLBusWrapperLocation = PBUS
)

case object NICKey extends Field[Option[NICConfig]](None)
case object NICAttachKey extends Field[NICAttachParams](NICAttachParams())

trait HasNICParameters {
  implicit val p: Parameters
  val nicExternal = p(NICKey).get
  val inBufFlits = nicExternal.inBufFlits
  val outBufFlits = nicExternal.outBufFlits
  val nMemXacts = nicExternal.nMemXacts
  val maxAcquireBytes = nicExternal.maxAcquireBytes
  val ctrlQueueDepth = nicExternal.ctrlQueueDepth
  val usePauser = nicExternal.usePauser
  // val checksumOffload = nicExternal.checksumOffload
  val checksumOffload = false
  val packetMaxBytes = nicExternal.packetMaxBytes
  val nMaxCores = nicExternal.nMaxCores
}

abstract class NICLazyModule(implicit p: Parameters)
  extends LazyModule with HasNICParameters

abstract class NICModule(implicit val p: Parameters)
  extends Module with HasNICParameters

abstract class NICBundle(implicit val p: Parameters)
  extends Bundle with HasNICParameters

class PacketArbiter(arbN: Int, rr: Boolean = false)
  extends HellaPeekingArbiter(
    new StreamChannel(NET_IF_WIDTH), arbN,
    (ch: StreamChannel) => ch.last, rr = rr)

class IceNicSendIO extends Bundle {
  val req = Decoupled(UInt(NET_IF_WIDTH.W))
  val comp = Flipped(Decoupled(Bool()))
}

class IceNicRecvIO extends Bundle {
  val req = Decoupled(UInt(NET_IF_WIDTH.W))
  val comp = Flipped(Decoupled(UInt(NET_LEN_BITS.W)))
}

case class IceNicControllerParams(address: BigInt, beatBytes: Int, nCores: Int)

class IceNicControllerBundle(nCores: Int) extends Bundle {
  val core = Flipped(Decoupled(UInt(log2Ceil(nCores).W))) // hash result
  val send = new IceNicSendIO
  val recv = new IceNicRecvIO
  val macAddr = Input(UInt(ETH_MAC_BITS.W))
  val txcsumReq = Decoupled(new ChecksumRewriteRequest)
  val rxcsumRes = Flipped(Decoupled(new TCPChecksumOffloadResult))
  val csumEnable = Output(Bool())
  val zero = Output(Bool())

  // Per-queue polling word DMA update requests (optional)
  val poll_req = Decoupled(new PollWriteReq(8))
  val poll_print_enable = Output(Bool())
}

/*
 * Take commands from the CPU over TL2, expose as Queues
 */
class IceNicController(c: IceNicControllerParams)(implicit p: Parameters)
    extends RegisterRouter(RegisterRouterParams("ice-nic", Seq("ucb-bar,ice-nic"),
      c.address, beatBytes=c.beatBytes))
    with HasTLControlRegMap
    with HasInterruptSources
    with HasNICParameters {
  def nCores: Int = c.nCores
  // TODO(qxh): implement multi-queue here
  override def nInterrupts = 1 + nCores
  def tlRegmap(mapping: RegField.Map*): Unit = regmap(mapping:_*)
  override lazy val module = new IceNiCControllerModuleImp(this)
}

class IceNiCControllerModuleImp(outer: IceNicController)(implicit p: Parameters) extends LazyModuleImp(outer) with HasNICParameters {
  val nCores = outer.nCores
  val io = IO(new IceNicControllerBundle(nCores))

  val (req_start :: req_received :: Nil) = Enum(2)
  val (comp_start :: comp_received :: comp_waiting :: Nil) = Enum(3)
  val req_state = RegInit(req_start)
  val req_core = RegInit(0.U(log2Ceil(nCores).W))
  val comp_state = RegInit(comp_start)
  val comp_core = RegInit(0.U(log2Ceil(nCores).W))

  io.core.ready := req_state === req_start
  
  val req_to_comp_valid = req_state === req_received
  val req_to_comp_ready = comp_state === comp_start
  val req_to_comp_fire = req_to_comp_valid && req_to_comp_ready

  when (req_state === req_start && io.core.fire) {
    req_state := req_received
    req_core := io.core.bits
  }

  when (req_to_comp_fire) {
    // rss ready
    comp_core := req_core
    comp_state := comp_received
    req_state := req_start

    midas.targetutils.SynthesizePrintf(printf("[State #2.2] comp core locked: 0x%d\n", req_core))
  }

  when (io.recv.req.fire) {
    assert(comp_state === comp_received, "Icenet: state error")
    midas.targetutils.SynthesizePrintf(printf("[State #2.2] comp core locked: 0x%d\n", req_core))
    comp_state := comp_waiting
  }
  
  when (io.recv.comp.fire) {
    assert(comp_state === comp_waiting, "Icenet: state error")
    comp_state := comp_start
    midas.targetutils.SynthesizePrintf(printf("[State #7  ] comp core unlocked: 0x%d\n", comp_core))
    midas.targetutils.SynthesizePrintf(printf("[State #7  ] Writer comp received, len: 0x%x\n", io.recv.comp.bits))
  }

  require(nCores <= nMaxCores)
  val sendCompDown = WireInit(false.B)

  val qDepth = ctrlQueueDepth
  require(qDepth < (1 << 8))

  def queueCount[T <: Data](qio: QueueIO[T], depth: Int): UInt =
    TwoWayCounter(qio.enq.fire, qio.deq.fire, depth)

  // TODO(qxh): implement multi-send-queue

  // hold (len, addr) of packets that we need to send out
  val sendReqQueue = Module(new HellaQueue(qDepth)(UInt(NET_IF_WIDTH.W)))
  val sendReqCount = queueCount(sendReqQueue.io, qDepth)
  // hold addr of buffers we can write received packets into

  // multi-receive-queue
  val recvReqQueue = (0 until nCores).map(i => Module(new HellaQueue(qDepth)(UInt(NET_IF_WIDTH.W))))
  val recvReqEnq   = Wire(Vec(nCores, Decoupled(UInt(NET_IF_WIDTH.W))))
  val recvReqDeq   = Wire(Vec(nCores, Flipped(Decoupled(UInt(NET_IF_WIDTH.W)))))
  for (i <- 0 until nCores) yield {
    recvReqQueue(i).io.enq <> recvReqEnq(i)
    recvReqDeq(i) <> recvReqQueue(i).io.deq
  }

  val recvReqCount = recvReqQueue.map(q => queueCount(q.io, qDepth))
  // count number of sends completed
  val sendCompCount = TwoWayCounter(io.send.comp.fire, sendCompDown, qDepth)
  // hold length of received packets

  val recvCompQueue = (0 until nCores).map(i => Module(new HellaQueue(qDepth)(UInt(NET_LEN_BITS.W))))
  val recvCompEnq   = Wire(Vec(nCores, Decoupled(UInt(NET_LEN_BITS.W))))
  val recvCompDeq   = Wire(Vec(nCores, Flipped(Decoupled(UInt(NET_LEN_BITS.W)))))
  for (i <- 0 until nCores) yield {
    recvCompQueue(i).io.enq <> recvCompEnq(i)
    recvCompDeq(i) <> recvCompQueue(i).io.deq
  }
  val recvCompCount = recvCompQueue.map(q => queueCount(q.io, qDepth))

  // Polling-mode DMA doorbell word
  private val pollEnableMask = RegInit(0.U(nCores.W))
  private val pollPrintEnable = RegInit(false.B)
  private val pollAddr = RegInit(VecInit(Seq.fill(nCores)(0.U(64.W))))
  private val pollAddrWritten = RegInit(VecInit(Seq.fill(nCores)(0.U(64.W))))
  private val pollCfgValid = RegInit(VecInit(Seq.fill(nCores)(false.B)))
  private val pollLast = RegInit(VecInit(Seq.fill(nCores)(false.B)))
  private val pollDirty = RegInit(VecInit(Seq.fill(nCores)(false.B)))

  private def pollActive(i: Int): Bool = pollEnableMask(i) && pollAddr(i).orR
  private val pollDesired = Wire(Vec(nCores, Bool()))
  for (i <- 0 until nCores) {
    pollDesired(i) := recvCompCount(i) =/= 0.U
  }

  for (i <- 0 until nCores) {
    when (!pollActive(i)) {
      pollCfgValid(i) := false.B
      pollDirty(i) := false.B
    }.otherwise {
      when (pollAddrWritten(i) =/= pollAddr(i)) {
        pollCfgValid(i) := false.B
      }
      when (!pollCfgValid(i) || (pollLast(i) =/= pollDesired(i))) {
        pollDirty(i) := true.B
      }
    }
  }

  // Issue at most one DMA write request per cycle
  val dirtyVec = Wire(Vec(nCores, Bool()))
  for (i <- 0 until nCores) {
    dirtyVec(i) := pollActive(i) && pollDirty(i)
  }
  val hasDirty = dirtyVec.asUInt.orR
  val sel = PriorityEncoder(dirtyVec)
  io.poll_req.valid := hasDirty
  io.poll_req.bits.q := sel
  io.poll_req.bits.addr := pollAddr(sel)
  io.poll_req.bits.data := Mux(pollDesired(sel), 1.U, 0.U)
  io.poll_print_enable := pollPrintEnable

  when (io.poll_req.fire) {
    pollDirty(sel) := false.B
    pollLast(sel) := pollDesired(sel)
    pollCfgValid(sel) := true.B
    pollAddrWritten(sel) := pollAddr(sel)
  }

  val sendCompValid = sendCompCount > 0.U 
  val intMask = RegInit(0.U((1 + nCores).W))

  io.send.req <> sendReqQueue.io.deq
  // io.recv.req <> recvReqDeq(io.core)
  for (i <- 0 until nCores) {
    recvReqDeq(i).ready := (i.U === comp_core) && io.recv.req.ready && comp_state === comp_received
  }
  io.recv.req.valid := recvReqDeq(comp_core).valid && comp_state === comp_received
  io.recv.req.bits := recvReqDeq(comp_core).bits
  io.send.comp.ready := sendCompCount < qDepth.U
  // recvCompEnq(io.core) <> io.recv.comp
  for (i <- 0 until nCores) {
    recvCompEnq(i).valid := i.U === comp_core && io.recv.comp.valid
    recvCompEnq(i).bits := io.recv.comp.bits.asUInt
  }
  io.recv.comp.ready := recvCompEnq(comp_core).ready

  outer.interrupts(0) := sendCompValid && intMask(0)
  for (i <- 0 until nCores) {
    outer.interrupts(i + 1) := recvCompDeq(i).valid && intMask(i + 1)
  }

  val sendReqSpace = (qDepth.U - sendReqCount)
  val recvReqSpace = (0 until nCores).map(i => qDepth.U - recvReqCount(i))

  def sendCompRead = (ready: Bool) => {
    sendCompDown := sendCompValid && ready
    (sendCompValid, true.B)
  }

  val txcsumReqQueue = Module(new HellaQueue(qDepth)(UInt(49.W)))
  val rxcsumResQueue = (0 until nCores).map(i => Module(new HellaQueue(qDepth)(UInt(2.W))))
  val rxcsumResEnq   = Wire(Vec(nCores, Decoupled(UInt(2.W))))
  val rxcsumResDeq   = Wire(Vec(nCores, Flipped(Decoupled(UInt(2.W)))))
  for (i <- 0 until nCores) yield {
    rxcsumResQueue(i).io.enq <> rxcsumResEnq(i)
    rxcsumResDeq(i) <> rxcsumResQueue(i).io.deq
  }
  val csumEnable = RegInit(false.B)

  io.txcsumReq.valid := txcsumReqQueue.io.deq.valid
  io.txcsumReq.bits := txcsumReqQueue.io.deq.bits.asTypeOf(new ChecksumRewriteRequest)
  txcsumReqQueue.io.deq.ready := io.txcsumReq.ready

  // rxcsumResEnq(io.core).valid := io.rxcsumRes.valid
  // rxcsumResEnq(io.core).bits := io.rxcsumRes.bits.asUInt
  for (i <- 0 until nCores) {
    rxcsumResEnq(i).valid := i.U === comp_core && io.rxcsumRes.valid
    rxcsumResEnq(i).bits := io.rxcsumRes.bits.asUInt
  }
  io.rxcsumRes.ready := rxcsumResEnq(comp_core).ready
  io.csumEnable := csumEnable

  val zero = RegInit(false.B)
  io.zero := zero

  when (io.recv.req.fire) {
    midas.targetutils.SynthesizePrintf(printf("[State #2.3] slot req fire: core #0x%d, slot addr 0x%x|0x%x\n", 
      comp_core, recvReqDeq(comp_core).bits, io.recv.req.bits))
  }

  when (RegNext(io.recv.comp.fire)) {
    val tmp = RegNext(comp_core)
    midas.targetutils.SynthesizePrintf(printf("[State #8  ] Interrupt detection: %d, deq_valid: %d, intMask: %d\n", 
      recvCompDeq(tmp).valid, intMask(tmp + 1.U), recvCompDeq(tmp).valid && intMask(tmp + 1.U)))
  }

    outer.tlRegmap(
      0x00 -> Seq(RegField.w(NET_IF_WIDTH, sendReqQueue.io.enq)),
      0x08 -> Seq(RegField.r(1, sendCompRead)),
      0x10 -> Seq(
        RegField.r(8, sendReqSpace),
        RegField.r(8, sendCompCount)),
      0x18 -> Seq(RegField.r(ETH_MAC_BITS, io.macAddr)),
      0x20 -> Seq(RegField.w(49, txcsumReqQueue.io.enq)),
      0x28 -> Seq(RegField(1, csumEnable)),

     /*
      * multi-queue receive region mapper
      */

      0x30 -> (1 until nCores).foldLeft(Seq(RegField.w(NET_IF_WIDTH, recvReqEnq(0)))) {(p, k) => 
        p ++ Seq(RegField.w(NET_IF_WIDTH, recvReqEnq(k)))},
      0xB0 -> (1 until nCores).foldLeft(Seq(RegField.r(NET_LEN_BITS, recvCompDeq(0)))) {(p, k) => 
        p ++ Seq(RegField.r(NET_LEN_BITS, recvCompDeq(k)))},
      0xD0 -> (1 until nCores).foldLeft(Seq(RegField.r(8, recvReqSpace(0)), RegField.r(8, recvCompCount(0)))) {(p, k) => 
        p ++ Seq(RegField.r(8, recvReqSpace(k)), 
                 RegField.r(8, recvCompCount(k)))},
      0xF0 -> Seq(RegField(1 + nCores, intMask)),
      0xF4 -> (1 until nCores).foldLeft(Seq(RegField.r(2, rxcsumResDeq(0)))) {(p, k) => 
        p ++ Seq(RegField.r(2, rxcsumResDeq(k)))},
      
      0xF8 -> Seq(RegField(1, zero)),

      0x100 -> Seq(RegField(nCores, pollEnableMask)),
      0x108 -> Seq(RegField(1, pollPrintEnable)),
      0x110 -> (0 until nCores).foldLeft(Seq(RegField(64, pollAddr(0)))) { (p, k) =>
        if (k == 0) p else p ++ Seq(RegField(64, pollAddr(k)))
      },
    )
}

class IceNicSendPath(nInputTaps: Int = 0)(implicit p: Parameters)
    extends NICLazyModule {
  val reader = LazyModule(new StreamReader(
    nMemXacts, outBufFlits, maxAcquireBytes))
  val node = reader.node

  lazy val module = new Impl
  class Impl extends LazyModuleImp(this) {
    val io = IO(new Bundle {
      val send = Flipped(new IceNicSendIO)
      val tap = Flipped(Vec(nInputTaps, Decoupled(new StreamChannel(NET_IF_WIDTH))))
      val out = Decoupled(new StreamChannel(NET_IF_WIDTH))
      val rlimit = Input(new RateLimiterSettings)
      val csum = checksumOffload.option(new Bundle {
        val req = Flipped(Decoupled(new ChecksumRewriteRequest))
        val enable = Input(Bool())
      })
    })

    val readreq = reader.module.io.req
    io.send.req.ready := readreq.ready
    readreq.valid := io.send.req.valid
    readreq.bits.address := io.send.req.bits(47, 0)
    readreq.bits.length  := io.send.req.bits(62, 48)
    readreq.bits.partial := io.send.req.bits(63)
    io.send.comp <> reader.module.io.resp

    val preArbOut = if (checksumOffload) {
      val readerOut = reader.module.io.out
      val arb = Module(new PacketArbiter(2))
      val bufFlits = (packetMaxBytes - 1) / NET_IF_BYTES + 1
      val rewriter = Module(new ChecksumRewrite(NET_IF_WIDTH, bufFlits))
      val enable = io.csum.get.enable

      rewriter.io.req <> io.csum.get.req

      arb.io.in(0) <> rewriter.io.stream.out
      arb.io.in(1).valid := !enable && readerOut.valid
      arb.io.in(1).bits  := readerOut.bits
      rewriter.io.stream.in.valid := enable && readerOut.valid
      rewriter.io.stream.in.bits := readerOut.bits
      readerOut.ready := Mux(enable,
        rewriter.io.stream.in.ready, arb.io.in(1).ready)

      arb.io.out
    } else { reader.module.io.out }

    val unlimitedOut = if (nInputTaps > 0) {
      val bufWords = (packetMaxBytes - 1) / NET_IF_BYTES + 1
      val inputs = (preArbOut +: io.tap).map { in =>
        // The packet collection buffer doesn't allow sending the first flit
        // of a packet until the last flit is received.
        // This ensures that we don't lock the arbiter while waiting for data
        // to arrive, which could cause deadocks.
        val buffer = Module(new PacketCollectionBuffer(bufWords))
        buffer.io.in <> in
        buffer.io.out
      }
      val arb = Module(new PacketArbiter(inputs.size, rr = true))
      arb.io.in <> inputs
      arb.io.out
    } else { preArbOut }

    val limiter = Module(new RateLimiter(new StreamChannel(NET_IF_WIDTH)))
    limiter.io.in <> unlimitedOut
    limiter.io.settings := io.rlimit
    io.out <> limiter.io.out
  }
}

class IceNicWriter(implicit p: Parameters) extends NICLazyModule {
  val writer = LazyModule(new StreamWriter(nMemXacts, maxAcquireBytes))
  val node = writer.node

  lazy val module = new Impl
  class Impl extends LazyModuleImp(this) {
    val io = IO(new Bundle {
      val recv = Flipped(new IceNicRecvIO)
      val in = Flipped(Decoupled(new StreamChannel(NET_IF_WIDTH)))
      val length = Flipped(Valid(UInt(NET_LEN_BITS.W)))
    })

    // State #4.1: Writer RX packets receive
    when (io.in.valid && !RegNext(io.in.valid)) {
      midas.targetutils.SynthesizePrintf(printf("[State #4.1] Writer RX packets receive\n"))
    } .elsewhen (io.in.valid && io.in.bits.last) {
      // State #5: Writer RX packages finished
      midas.targetutils.SynthesizePrintf(printf("[State #5  ] Writer RX packets finished\n"))
    }

    // State #6: Writer RX packets writing completed
    when (io.recv.comp.fire) {
      midas.targetutils.SynthesizePrintf(printf("[State #6  ] Writer RX packets writing completed, len: 0x%x\n", io.recv.comp.bits))
    } .elsewhen (io.recv.comp.valid) {
      midas.targetutils.SynthesizePrintf(printf("[State #6??] Writer RX packets writing completed, len: 0x%x\n", io.recv.comp.bits))
    }

    val streaming = RegInit(false.B)
    val byteAddrBits = log2Ceil(NET_IF_BYTES)
    val helper = DecoupledHelper(
      io.recv.req.valid,
      writer.module.io.req.ready,
      io.length.valid, !streaming)

    writer.module.io.req.valid := helper.fire(writer.module.io.req.ready)
    writer.module.io.req.bits.address := io.recv.req.bits
    writer.module.io.req.bits.length := io.length.bits
    io.recv.req.ready := helper.fire(io.recv.req.valid)

    writer.module.io.in.valid := io.in.valid && streaming
    writer.module.io.in.bits := io.in.bits
    io.in.ready := writer.module.io.in.ready && streaming

    io.recv.comp <> writer.module.io.resp

    when (io.recv.req.fire) { 
      midas.targetutils.SynthesizePrintf(printf("[State #4.0] Writer slot req fire addr: 0x%x\n", io.recv.req.bits))
      streaming := true.B 
    }
    when (io.in.fire && io.in.bits.last) { streaming := false.B }
  }
}

/*
 * Recv frames
 */
class IceNicRecvPath(val tapFuncs: Seq[EthernetHeader => Bool] = Nil, nCores: Int = 2, random: Boolean = false)
    (implicit p: Parameters) extends LazyModule {
  val writer = LazyModule(new IceNicWriter)
  val node = TLIdentityNode()
  node := writer.node
  lazy val module = new IceNicRecvPathModule(this, nCores, random)
}

class IceNicRecvPathModule(val outer: IceNicRecvPath, nCores: Int, random: Boolean)
    extends LazyModuleImp(outer) with HasNICParameters {
  def usingRSS = nCores > 1

  val io = IO(new Bundle {
    val recv = Flipped(new IceNicRecvIO)
    val in = Flipped(Decoupled(new StreamChannel(NET_IF_WIDTH))) // input stream
    val tap = Vec(outer.tapFuncs.length, Decoupled(new StreamChannel(NET_IF_WIDTH)))
    val csum = checksumOffload.option(new Bundle {
      val res = Decoupled(new TCPChecksumOffloadResult)
      val enable = Input(Bool())
    })
    val buf_free = Output(Vec(1 + outer.tapFuncs.length, UInt(8.W)))
    val hash_core = Decoupled(UInt(log2Ceil(nCores).W))
    val zero = Input(Bool())
  })

  def tapOutToDropCheck(tapOut: EthernetHeader => Bool) = {
    (header: EthernetHeader, ch: StreamChannel, update: Bool) => {
      val first = RegInit(true.B)
      val drop = tapOut(header) && first
      val dropReg = RegInit(false.B)

      when (update && first) { first := false.B; dropReg := drop }
      when (update && ch.last) { first := true.B; dropReg := false.B }

      drop || dropReg
    }
  }

  def duplicateStream(in: DecoupledIO[StreamChannel], outs: Seq[DecoupledIO[StreamChannel]]) = {
    outs.foreach { out =>
      out.valid := in.valid
      out.bits := in.bits
    }
    in.ready := outs.head.ready
    val outReadys = Cat(outs.map(_.ready))
    assert(outReadys.andR || !outReadys.orR,
      "Duplicated streams must all be ready simultaneously")
    outs
  }

  def invertCheck(check: (EthernetHeader, StreamChannel, Bool) => Bool) =
    (eth: EthernetHeader, ch: StreamChannel, up: Bool) => !check(eth, ch, up)

  val tapDropChecks = outer.tapFuncs.map(func => tapOutToDropCheck(func))
  val pauseDropCheck = if (usePauser) Some(PauseDropCheck(_, _, _)) else None
  val allDropChecks =
    // Drop checks for the primary buffer
    // Drop if the packet should be tapped out or is a pause frame
    Seq(tapDropChecks ++ pauseDropCheck.toSeq) ++
    // Drop checks for the tap buffers
    // For each tap, drop if the packet doesn't match the tap function or is a pause frame
    tapDropChecks.map(check => invertCheck(check) +: pauseDropCheck.toSeq)
  
  val (core: UInt, hash_valid: Bool) = (if (usingRSS) {
    val rss = Module(new RSS(nCores = nCores, random = random))
    rss.io.in.valid := io.in.valid
    rss.io.in.bits  := io.in.bits
    (rss.io.hash_core.bits, rss.io.hash_core.valid)
  } else { (0.U, true.B) })

  // State #1: RX packets receive
  when (io.in.valid && !RegNext(io.in.valid)) {
    midas.targetutils.SynthesizePrintf(printf("[State #1  ] RX packets receive\n"))
  } .elsewhen (io.in.valid && io.in.bits.last) {
    // State #3: RX packages finished
    midas.targetutils.SynthesizePrintf(printf("[State #3  ] RX packages finished, RSS 0x%d\n", core))
  }

  // State #2.1: RSS ready
  when (hash_valid && !RegNext(hash_valid)) {
    midas.targetutils.SynthesizePrintf(printf("[State #2.1] RSS ready 0x%d\n", core))
  }

  val buffers = allDropChecks.map(dropChecks =>
    Module(new NetworkPacketBuffer(
      inBufFlits,
      maxBytes = packetMaxBytes,
      dropChecks = dropChecks, dropless = usePauser)))
  duplicateStream(io.in, buffers.map(_.io.stream.in))

  io.buf_free := buffers.map(_.io.free)

  io.tap <> buffers.tail.map(_.io.stream.out)
  val bufout = buffers.head.io.stream.out
  val buflen = buffers.head.io.length

  // TODO(qxh): does it lead to incorrect enq_fire for recvReqCount?

  val (csumout, recvreq) = (if (checksumOffload) {
    val offload = Module(new TCPChecksumOffload(NET_IF_WIDTH))
    val offloadReady = offload.io.in.ready || !io.csum.get.enable

    val out = Wire(Decoupled(new StreamChannel(NET_IF_WIDTH)))
    val recvreq = Wire(Decoupled(UInt(NET_IF_WIDTH.W)))
    val reqq = Module(new Queue(UInt(NET_IF_WIDTH.W), 1))

    val enqHelper = DecoupledHelper(
      io.recv.req.valid, reqq.io.enq.ready, recvreq.ready)
    val deqHelper = DecoupledHelper(
      bufout.valid, offloadReady, out.ready, reqq.io.deq.valid)

    reqq.io.enq.valid := enqHelper.fire(reqq.io.enq.ready)
    reqq.io.enq.bits := io.recv.req.bits
    io.recv.req.ready := enqHelper.fire(io.recv.req.valid)
    recvreq.valid := enqHelper.fire(recvreq.ready, hash_valid)
    recvreq.bits := io.recv.req.bits

    out.valid := deqHelper.fire(out.ready)
    out.bits  := bufout.bits
    offload.io.in.valid := deqHelper.fire(offloadReady, io.csum.get.enable)
    offload.io.in.bits := bufout.bits
    bufout.ready := deqHelper.fire(bufout.valid)
    reqq.io.deq.ready := deqHelper.fire(reqq.io.deq.valid, bufout.bits.last)

    io.csum.get.res <> offload.io.result

    (out, recvreq)
  } else { (bufout, io.recv.req) })

  val writer = outer.writer.module
  writer.io.recv.req <> Queue(recvreq, 1)
  writer.io.in <> csumout
  writer.io.length.valid := buflen.valid
  writer.io.length.bits  := buflen.bits

  io.recv.comp <> writer.io.recv.comp
  // if rss isn't ready when packets end, then send the packet to core#0
  io.hash_core.bits := Mux(io.zero || (io.in.bits.last && !hash_valid), 0.U, core)
  io.hash_core.valid := Mux(io.in.bits.last && !hash_valid, true.B, hash_valid)
}

class NICIO extends StreamIO(NET_IF_WIDTH) {
  val macAddr = Input(UInt(ETH_MAC_BITS.W))
  val rlimit = Input(new RateLimiterSettings)
  val pauser = Input(new PauserSettings)

}

/*
 * A simple NIC
 *
 * Expects ethernet frames (see below), but uses a custom transport
 * (see ExtBundle)
 *
 * Ethernet Frame format:
 *   2 bytes |  6 bytes  |  6 bytes    | 2 bytes  | 46-1500B
 *   Padding | Dest Addr | Source Addr | Type/Len | Data
 *
 * @address Starting address of MMIO control registers
 * @beatBytes Width of memory interface (in bytes)
 * @tapOutFuncs Sequence of functions for each output tap.
 *              Each function takes the header of an Ethernet frame
 *              and returns Bool that is true if matching and false if not.
 * @nInputTaps Number of input taps
 * @nCores Number of CPU cores
 *
 */
class IceNIC(address: BigInt, beatBytes: Int = 8,
    tapOutFuncs: Seq[EthernetHeader => Bool] = Nil,
    nInputTaps: Int = 0, nCores: Int = 1)
    (implicit p: Parameters) extends NICLazyModule {

  val control = LazyModule(new IceNicController(
    IceNicControllerParams(address, beatBytes, nCores)))
  val sendPath = LazyModule(new IceNicSendPath(nInputTaps))
  val recvPath = LazyModule(new IceNicRecvPath(tapOutFuncs, nCores))
  val pollWriter = LazyModule(new PollWriter())

  val mmionode = TLIdentityNode()
  val dmanode = TLIdentityNode()
  val intnode = control.intXing(NoCrossing)
  // val testnode = IntSinkNode(IntSinkPortSimple())

  control.node := TLAtomicAutomata() := mmionode
  dmanode := TLWidthWidget(NET_IF_BYTES) := sendPath.node
  dmanode := TLWidthWidget(NET_IF_BYTES) := recvPath.node
  dmanode := TLWidthWidget(NET_IF_BYTES) := pollWriter.node

  lazy val module = new Impl
  class Impl extends LazyModuleImp(this) {
    val io = IO(new Bundle {
      val ext = new NICIO
      val tapOut = Vec(tapOutFuncs.length, Decoupled(new StreamChannel(NET_IF_WIDTH)))
      val tapIn = Flipped(Vec(nInputTaps, Decoupled(new StreamChannel(NET_IF_WIDTH))))
    })
    // TODO(qxh): implement multi-send-queue
    sendPath.module.io.send <> control.module.io.send
    recvPath.module.io.recv <> control.module.io.recv
    control.module.io.core <> recvPath.module.io.hash_core
    recvPath.module.io.zero := control.module.io.zero

    // val start_test = testnode.in.head._1.asUInt.orR

    // connect externally
    if (usePauser) {
      val pauser = Module(new Pauser(inBufFlits, 1 + tapOutFuncs.length))
      pauser.io.int.out <> sendPath.module.io.out
      recvPath.module.io.in <> pauser.io.int.in
      io.ext.out <> pauser.io.ext.out
      pauser.io.ext.in <> io.ext.in
      pauser.io.in_free := recvPath.module.io.buf_free
      pauser.io.macAddr := io.ext.macAddr
      pauser.io.settings := io.ext.pauser
    } else {
      recvPath.module.io.in <> io.ext.in
      io.ext.out <> sendPath.module.io.out
    }

    control.module.io.macAddr := io.ext.macAddr
    sendPath.module.io.rlimit := io.ext.rlimit

    pollWriter.module.io.req <> control.module.io.poll_req
    pollWriter.module.io.print_enable := control.module.io.poll_print_enable

    io.tapOut <> recvPath.module.io.tap
    sendPath.module.io.tap <> io.tapIn

    if (checksumOffload) {
      sendPath.module.io.csum.get.req <> control.module.io.txcsumReq
      sendPath.module.io.csum.get.enable := control.module.io.csumEnable
      control.module.io.rxcsumRes <> recvPath.module.io.csum.get.res
      recvPath.module.io.csum.get.enable := control.module.io.csumEnable
    } else {
      control.module.io.txcsumReq.ready := false.B
      control.module.io.rxcsumRes.valid := false.B
      control.module.io.rxcsumRes.bits := DontCare
    }
  }
}

class SimNetwork extends BlackBox with HasBlackBoxResource {
  val io = IO(new Bundle {
    val clock = Input(Clock())
    val reset = Input(Bool())
    val net = Flipped(new NICIOvonly)
  })
  addResource("/vsrc/SimNetwork.v")
  addResource("/csrc/SimNetwork.cc")
  addResource("/csrc/device.h")
  addResource("/csrc/device.cc")
  addResource("/csrc/switch.h")
  addResource("/csrc/switch.cc")
  addResource("/csrc/packet.h")
}


class NICIOvonly extends Bundle {
  val in = Flipped(Valid(new StreamChannel(NET_IF_WIDTH)))
  val out = Valid(new StreamChannel(NET_IF_WIDTH))
  val macAddr = Input(UInt(ETH_MAC_BITS.W))
  val rlimit = Input(new RateLimiterSettings)
  val pauser = Input(new PauserSettings)

}

object NICIOvonly {
  def apply(nicio: NICIO): NICIOvonly = {
    val vonly = Wire(new NICIOvonly)
    vonly.out.valid := nicio.out.valid
    vonly.out.bits  := nicio.out.bits
    nicio.out.ready := true.B
    nicio.in.valid  := vonly.in.valid
    nicio.in.bits   := vonly.in.bits
    assert(!vonly.in.valid || nicio.in.ready, "NIC input not ready for valid")
    nicio.macAddr := vonly.macAddr
    nicio.rlimit  := vonly.rlimit
    nicio.pauser  := vonly.pauser
    vonly
  }
}

object NICIO {
  def apply(vonly: NICIOvonly): NICIO = {
    val nicio = Wire(new NICIO)
    assert(!vonly.out.valid || nicio.out.ready)
    nicio.out.valid := vonly.out.valid
    nicio.out.bits  := vonly.out.bits
    vonly.in.valid  := nicio.in.valid
    vonly.in.bits   := nicio.in.bits
    nicio.in.ready  := true.B
    vonly.macAddr   := nicio.macAddr
    vonly.rlimit    := nicio.rlimit
    vonly.pauser    := nicio.pauser
    nicio
  }

}
trait CanHavePeripheryIceNIC  { this: BaseSubsystem 
  with InstantiatesHierarchicalElements 
  with HasTileNotificationSinks =>
  private val address = BigInt(0x10016000)
  private val portName = "Ice-NIC"


  val icenicOpt = p(NICKey).map { params =>
    val manager = locateTLBusWrapper(p(NICAttachKey).slaveWhere)
    val client = locateTLBusWrapper(p(NICAttachKey).masterWhere)
    // TODO: currently the controller is in the clock domain of the bus which masters it
    // we assume this is same as the clock domain of the bus the controller masters
    val domain = manager.generateSynchronousDomain.suggestName("icenic_domain")
    // val nicNode = totalTiles.values.map {
    //   case r: RocketTile => r.nicNode
    // }.toList.head
    // val tileNICXbarNode = IntXbar()
    // val tileNICSinkNode = IntSinkNode(IntSinkPortSimple())

    // assert(nTotalTiles == 1 && tile_prci_domains.size == 1)
    // val rocket_domain: TilePRCIDomain[RocketTile] = tile_prci_domains.values.toSeq.head.asInstanceOf[TilePRCIDomain[RocketTile]];
    // val rocket_domain = tile_prci_domains.values.collectFirst {
    //   case domain: TilePRCIDomain[RocketTile] => domain
    // }.getOrElse(
    //   throw new Exception("Expected a RocketTile PRCI domain but none found.")
    // )
    // tileNICXbarNode :=* nicNode
    // tileNICSinkNode := tileNICXbarNode

    val icenic = domain { LazyModule(new IceNIC(address = address, beatBytes = manager.beatBytes, nCores = nTotalTiles)) }

    manager.coupleTo(portName) { icenic.mmionode := TLFragmenter(manager.beatBytes, manager.blockBytes) := _ }
    client.coupleFrom(portName) { _ :=* icenic.dmanode }
    ibus.fromSync := icenic.intnode
    // icenic.testnode := tileNICXbarNode

    val inner_io = domain { InModuleBody {
      val inner_io = IO(new NICIOvonly).suggestName("nic")
      inner_io <> NICIOvonly(icenic.module.io.ext)
      // icenic.module.io.start_test := tileNICSinkNode.in.head._1.asUInt.orR
      inner_io
    } }

    val outer_io = InModuleBody {
      val outer_io = IO(new ClockedIO(new NICIOvonly)).suggestName("nic")
      outer_io.bits <> inner_io
      outer_io.clock := domain.module.clock
      outer_io
    }
    outer_io
  }
}


object NicLoopback {
  def connect(net: Option[NICIOvonly], nicConf: Option[NICConfig], qDepth: Int, latency: Int = 10): Unit = {
    net.foreach { netio =>
      import PauseConsts.BT_PER_QUANTA
      val packetWords = nicConf.get.packetMaxBytes / NET_IF_BYTES
      val packetQuanta = (nicConf.get.packetMaxBytes * 8) / BT_PER_QUANTA
      netio.macAddr := PlusArg("macaddr")
      netio.rlimit.inc := PlusArg("rlimit-inc", 1)
      netio.rlimit.period := PlusArg("rlimit-period", 1)
      netio.rlimit.size := PlusArg("rlimit-size", 8)
      netio.pauser.threshold := PlusArg("pauser-threshold", 2 * packetWords + latency)
      netio.pauser.quanta := PlusArg("pauser-quanta", 2 * packetQuanta)
      netio.pauser.refresh := PlusArg("pauser-refresh", packetWords)

      if (nicConf.get.usePauser) {
        val pauser = Module(new PauserComplex(qDepth))
        pauser.io.ext.flipConnect(NetDelay(NICIO(netio), latency))
        pauser.io.int.out <> pauser.io.int.in
        pauser.io.macAddr := netio.macAddr + (1 << 40).U
        pauser.io.settings := netio.pauser
      } else {

        netio.in := Pipe(netio.out, latency)
      }
      netio.in.bits.keep := NET_FULL_KEEP
    }
  }

  def connect(net: NICIOvonly, nicConf: NICConfig): Unit = {
    val packetWords = nicConf.packetMaxBytes / NET_IF_BYTES
    NicLoopback.connect(Some(net), Some(nicConf), 4 * packetWords)
  }
}

object SimNetwork {
  def connect(net: Option[NICIOvonly], clock: Clock, reset: Bool) {
    net.foreach { netio =>
      val sim = Module(new SimNetwork)
      sim.io.clock := clock
      sim.io.reset := reset
      sim.io.net <> netio
    }
  }
}
