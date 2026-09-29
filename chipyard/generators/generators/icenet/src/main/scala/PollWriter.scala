package icenet

import chisel3._
import chisel3.util._
import freechips.rocketchip.diplomacy._
import freechips.rocketchip.tilelink._
import org.chipsalliance.cde.config.Parameters

/** A minimal TL DMA writer for the per-queue polling word **/
class PollWriteReq(qBits: Int) extends Bundle {
  val q = UInt(qBits.W)
  val addr = UInt(64.W)
  val data = UInt(64.W)
}

class PollWriter(nXacts: Int = 1)(implicit p: Parameters) extends LazyModule {
  val node = TLClientNode(Seq(TLMasterPortParameters.v1(Seq(TLClientParameters(
    name = "poll-writer", sourceId = IdRange(0, nXacts)
  )))))

  lazy val module = new Impl
  class Impl extends LazyModuleImp(this) {
    val (tl, edge) = node.out(0)
    val dataBits = tl.params.dataBits
    val beatBytes = dataBits / 8
    val byteAddrBits = log2Ceil(beatBytes)

    require(beatBytes >= 8, "poll-writer requires beatBytes >= 8")
    require((beatBytes % 8) == 0, "poll-writer expects beatBytes to be a multiple of 8")

    val qBits = 8
    val io = IO(new Bundle {
      val req = Flipped(Decoupled(new PollWriteReq(qBits)))
      val print_enable = Input(Bool())
    })

    val sIdle :: sWaitD :: Nil = Enum(2)
    val state = RegInit(sIdle)

    val r_q = Reg(UInt(qBits.W))
    val r_addr = Reg(UInt(64.W))
    val r_data = Reg(UInt(64.W))

    val addrBits = tl.params.addressBits

    val reqAddr = io.req.bits.addr(addrBits - 1, 0)
    val reqByteOff = io.req.bits.addr(byteAddrBits - 1, 0)
    val reqBaseAddr = reqAddr & (~(BigInt(beatBytes - 1).U(addrBits.W)))

    // Place the 8-byte data into the correct position within the beat.
    val reqShiftBits = (reqByteOff << 3).asUInt
    val reqDataPadded =
      if (dataBits == 64) io.req.bits.data
      else Cat(0.U((dataBits - 64).W), io.req.bits.data)
    val reqDataBeat = (reqDataPadded << reqShiftBits)(dataBits - 1, 0)

    val mask8 = ((BigInt(1) << 8) - 1).U(beatBytes.W)
    val reqMask = (mask8 << reqByteOff)(beatBytes - 1, 0)

    val put = edge.Put(
      fromSource = 0.U,
      toAddress = reqBaseAddr,
      lgSize = log2Ceil(beatBytes).U,
      data = reqDataBeat,
      mask = reqMask)._2

    // No restrictions on buffers here, keep it simple.
    tl.a.valid := (state === sIdle) && io.req.valid
    tl.a.bits := put
    io.req.ready := (state === sIdle) && tl.a.ready

    tl.d.ready := (state === sWaitD)

    when (io.req.fire) {
      assert((io.req.bits.addr(2, 0) === 0.U), "poll-writer requires 8-byte aligned addr")
      r_q := io.req.bits.q
      r_addr := io.req.bits.addr
      r_data := io.req.bits.data
      state := sWaitD
    }

    when (state === sWaitD && tl.d.fire) {
      midas.targetutils.SynthesizePrintf(printf(
          "[icenet][poll] dma_write q=%d addr=0x%x data=0x%x\n",
          r_q, r_addr, r_data))
      state := sIdle
    }
  }
}
