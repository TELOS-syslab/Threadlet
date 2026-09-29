// See LICENSE.SiFive for license details.

package freechips.rocketchip.util

import chisel3._
import chisel3.util._

/** Implements the same interface as chisel3.util.Queue, but uses a shift
  * register internally.  It is less energy efficient whenever the queue
  * has more than one entry populated, but is faster on the dequeue side.
  * It is efficient for usually-empty flow-through queues. */
class ShiftQueue[T <: Data](gen: T,
                            val entries: Int,
                            pipe: Boolean = false,
                            flow: Boolean = false,
                            threadIdLength : Int = 4,
                            threadSupport: Boolean = false)
    extends Module {
  val io = IO(new QueueIO(gen, entries) {
    val mask = Output(UInt(entries.W))
    val kill = Flipped(Valid(UInt(threadIdLength.W)))
    val ithread = Input(UInt(threadIdLength.W))
    val othread = Output(UInt(threadIdLength.W))
  })

  private val valid = RegInit(VecInit(Seq.fill(entries) { false.B }))
  private val elts = Reg(Vec(entries, gen))
  private val thread = RegInit(VecInit(Seq.fill(entries) { 0.U(threadIdLength.W) }))

  for (i <- 0 until entries) {
    def paddedValid(i: Int) = if (i == -1) true.B else if (i == entries) false.B else valid(i)

    val wdata = if (i == entries-1) io.enq.bits else Mux(valid(i+1), elts(i+1), io.enq.bits)
    val wthread = if (i == entries-1) io.ithread else Mux(valid(i+1), thread(i+1), io.ithread)
    val wen =
      Mux(io.deq.ready,
          paddedValid(i+1) || io.enq.fire && ((i == 0 && !flow).B || valid(i)),
          io.enq.fire && paddedValid(i-1) && !valid(i))
    val current_thread = Mux(wen, wthread, thread(i))
    when (wen) {
      elts(i) := wdata
      thread(i) := wthread
    }

    valid(i) :=
      Mux(io.kill.valid && io.kill.bits === current_thread,
        false.B,
        Mux(io.deq.ready,
            paddedValid(i+1) || io.enq.fire && ((i == 0 && !flow).B || valid(i)),
            io.enq.fire && paddedValid(i-1) || valid(i)))
  }

  io.enq.ready := !valid(entries-1)
  io.deq.valid := valid(0)
  io.deq.bits := elts.head
  io.othread := thread(0)

  if (flow) {
    when (io.enq.valid) { io.deq.valid := true.B }
    when (!valid(0)) { 
      io.deq.bits := io.enq.bits
      io.othread := io.ithread
    }
  }

  if (pipe) {
    when (io.deq.ready) { io.enq.ready := true.B }
  }

  io.mask := valid.asUInt
  io.count := PopCount(io.mask)
}

object ShiftQueue
{
  def apply[T <: Data](enq: DecoupledIO[T], entries: Int = 2, pipe: Boolean = false, flow: Boolean = false): DecoupledIO[T] = {
    val q = Module(new ShiftQueue(enq.bits.cloneType, entries, pipe, flow))
    q.io.enq <> enq
    q.io.deq
  }
}
