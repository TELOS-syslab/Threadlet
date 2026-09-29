package freechips.rocketchip.rocket

import chisel3._
import chisel3.util._
import org.chipsalliance.cde.config.Parameters
import freechips.rocketchip.util._
import freechips.rocketchip.tile.{CoreBundle, CoreModule, BaseTile}
import org.chipsalliance.cde.config._

import Instructions._
import CustomInstructions._
import ALU._

object ThreadInstructions {
  def THREAD_INIT         = BitPat("b00000000000000000000000000001011")
  def THREAD_CREATE       = BitPat("b000000100000?????000?????0001011")
  def THREAD_HALT         = BitPat("b00000100000000000000000000001011")
  def THREAD_TEST         = BitPat("b00000110000000000000000000001011")
  def THREAD_CTX_WRITE    = BitPat("b0000100??????????000?????0001011")
  def THREAD_SET_PROI     = BitPat("b0000101??????????000000000001011")
  def THREAD_SET_SLICE    = BitPat("b0010101??????????000000000001011")
  def THREAD_SET_DEADLINE = BitPat("b0010110??????????000000000001011")
  def THREAD_PASS         = BitPat("b00101110000000000000000000001011")
  def THREAD_SET_CTX_BASE = BitPat("b001100100000?????000000000001011")
  def THREAD_CURRENT      = BitPat("b00001110000000000000?????0001011")
  def THREAD_YIELD        = BitPat("b00010000000000000000000000001011")
  def THREAD_WAKEUP       = BitPat("b000101000000?????000000000001011")
  def THREAD_CSR_SET      = BitPat("b000101100000?????000000000001011")
  def THREAD_SYNPRINT_EN  = BitPat("b00011000000000000000000000001011")
  def THREAD_SYNPRINT_DIS = BitPat("b00011010000000000000000000001011")
  def THREAD_SYN_PRINT    = BitPat("b0001110??????????000000000001011")
  def THREAD_SET_BASE     = BitPat("b000111100000?????000000000001011")
  def THREAD_DCACHE_PRINT_EN  = BitPat("b00100000000000000000000000001011")
  def THREAD_DCACHE_PRINT_DIS = BitPat("b00100010000000000000000000001011")
  def THREAD_DCACHE_MONITOR_SET   = BitPat("b0010010??????????000000000001011")
  def THREAD_DCACHE_MONITOR_CLEAR = BitPat("b0010011??????????000000000001011")
  def THREAD_DCACHE_MWAIT         = BitPat("b0010100??????????000000000001011")
  def THREAD_ERET         = BitPat("b00000000000000000001000000001011")
}

import ThreadInstructions._

class ThreadCtrlSigs(implicit val p: Parameters) extends Bundle {
  val legal = Bool()
  val init = Bool()
  val create = Bool()
  val halt = Bool()
  val test = Bool()
  val ctx_write = Bool()
  val set_prior = Bool()
  val set_slice = Bool()
  val set_deadline = Bool()
  val get_prior = Bool()
  val current = Bool()
  val yields = Bool()
  val status = Bool()
  val wakeup = Bool()
  val csr_set = Bool()
  val enable = Bool()
  val disable = Bool()
  val dcache_print_enable = Bool()
  val dcache_print_disable = Bool()
  val dcache_monitor_set = Bool()
  val dcache_monitor_clear = Bool()
  val syn_print = Bool()
  val set_base = Bool()
  val eret = Bool()
  val pass = Bool()
  val register_event_wakeup = Bool()
  val set_ctx_base = Bool()

  def toSeq: Seq[Bool] = Seq(legal, init, create, halt, test, 
    ctx_write, set_prior, set_slice, set_deadline, get_prior, current, yields, status, wakeup, 
    csr_set, enable, disable, dcache_print_enable, dcache_print_disable,
    dcache_monitor_set, dcache_monitor_clear, syn_print, set_base, eret,
    pass, register_event_wakeup, set_ctx_base)
}

class ThreadInstCtrlSigs(implicit val p: Parameters) extends Bundle {
  
  val ctrl = new ThreadCtrlSigs

	  def default: List[BitPat] =
	                //                                 set_prior   
	                //                 halt              | get_prior       status         enable
	                //          create |                 |   |   current    |               | disable         eret
	                //  legal   |      |      ctx_write  |   |    |         |   wakeup      |   | syn_print    |
	                //   | init |      |   test |        |   |    |  yields |    |  csr_set |   |   | dcache_en/dis | set_base |
	                //   |  |   |      |     |  |        |   |    |    |    |    |   |      |   |   |      |      |  |       |
		                List(N, X,  X,     X,    X, X,       X,  X,  X,   X,   X,   X,   X,  X,     X,  X,  X, X,       X, X, X,   X, X, X, X, X, X)

  def decode(inst: UInt, table: Iterable[(BitPat, List[BitPat])]) = {
    val decoder = DecodeLogic(inst, default, table)
    val sigs = ctrl.toSeq
    sigs zip decoder map {case(s,d) => s := d}
    this
  }
}

class ThreadDecode(implicit val p: Parameters) extends DecodeConstants {
  val table: Array[(BitPat, List[BitPat])] = Array(
    THREAD_INIT        -> List(Y,N,N,N,N,N,N,X,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
    THREAD_CREATE      -> List(Y,N,N,N,N,N,N,Y,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,Y,CSR.N,N,N,N,N),
    THREAD_HALT        -> List(Y,N,N,N,N,N,N,X,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
    THREAD_TEST        -> List(Y,N,N,N,N,N,N,X,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
    THREAD_CTX_WRITE   -> List(Y,N,N,N,N,N,Y,Y,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,Y,CSR.N,N,N,N,N),
    THREAD_SET_PROI    -> List(Y,N,N,N,N,N,Y,Y,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
    THREAD_SET_SLICE   -> List(Y,N,N,N,N,N,Y,Y,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
    THREAD_SET_DEADLINE-> List(Y,N,N,N,N,N,Y,Y,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
    THREAD_PASS        -> List(Y,N,N,N,N,N,N,X,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
    THREAD_SET_CTX_BASE -> List(Y,N,N,N,N,N,N,Y,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
    THREAD_CURRENT     -> List(Y,N,N,N,N,N,N,X,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,Y,CSR.N,N,N,N,N),
    THREAD_YIELD       -> List(Y,N,N,N,N,N,N,X,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
    THREAD_WAKEUP      -> List(Y,N,N,N,N,N,N,Y,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
    THREAD_CSR_SET     -> List(Y,N,N,N,N,N,N,Y,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
    THREAD_SYNPRINT_EN -> List(Y,N,N,N,N,N,N,X,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
    THREAD_SYNPRINT_DIS-> List(Y,N,N,N,N,N,N,X,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
    THREAD_DCACHE_PRINT_EN  -> List(Y,N,N,N,N,N,N,X,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
    THREAD_DCACHE_PRINT_DIS -> List(Y,N,N,N,N,N,N,X,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),


    THREAD_DCACHE_MONITOR_SET   -> List(Y,N,N,N,N,N,Y,Y,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
    THREAD_DCACHE_MONITOR_CLEAR -> List(Y,N,N,N,N,N,Y,Y,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
    THREAD_DCACHE_MWAIT         -> List(Y,N,N,N,N,N,Y,Y,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
    THREAD_SYN_PRINT   -> List(Y,N,N,N,N,N,Y,Y,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
    THREAD_SET_BASE    -> List(Y,N,N,N,N,N,N,Y,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
    THREAD_ERET        -> List(Y,N,N,N,N,N,N,N,A2_X,   A1_X,   IMM_X, DW_X,  FN_X,     N,M_X,        N,N,N,N,N,N,N,CSR.N,N,N,N,N),
  )
}

class ThreadDecodeSelf(implicit val p: Parameters) extends DecodeConstants {
  val table: Array[(BitPat, List[BitPat])] = Array(
    THREAD_INIT        -> List(Y, Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N),
    THREAD_CREATE      -> List(Y, N, Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N),
    THREAD_HALT        -> List(Y, N, N, Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N),
    THREAD_TEST        -> List(Y, N, N, N, Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N),
    THREAD_CTX_WRITE   -> List(Y, N, N, N, N, Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N),
    THREAD_SET_PROI    -> List(Y, N, N, N, N, N, Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N),
    THREAD_SET_SLICE   -> List(Y, N, N, N, N, N, N, Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N),
    THREAD_SET_DEADLINE-> List(Y, N, N, N, N, N, N, N, Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N),
    THREAD_PASS        -> List(Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, Y, N, N),
    THREAD_SET_CTX_BASE -> List(Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, Y),
    THREAD_CURRENT     -> List(Y, N, N, N, N, N, N, N, N, N, Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N),
    THREAD_YIELD       -> List(Y, N, N, N, N, N, N, N, N, N, N, Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N),
    THREAD_WAKEUP      -> List(Y, N, N, N, N, N, N, N, N, N, N, N, N, Y, N, N, N, N, N, N, N, N, N, N, N, N, N),
    THREAD_CSR_SET     -> List(Y, N, N, N, N, N, N, N, N, N, N, N, N, N, Y, N, N, N, N, N, N, N, N, N, N, N, N),
    THREAD_SYNPRINT_EN -> List(Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, Y, N, N, N, N, N, N, N, N, N, N, N),
    THREAD_SYNPRINT_DIS-> List(Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, Y, N, N, N, N, N, N, N, N, N, N),
    THREAD_DCACHE_PRINT_EN  -> List(Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, Y, N, N, N, N, N, N, N, N, N),
    THREAD_DCACHE_PRINT_DIS -> List(Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, Y, N, N, N, N, N, N, N, N),
    THREAD_DCACHE_MONITOR_SET   -> List(Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, Y, N, N, N, N, N, N, N),
    THREAD_DCACHE_MONITOR_CLEAR -> List(Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, Y, N, N, N, N, N, N),
    THREAD_DCACHE_MWAIT         -> List(Y, N, N, N, N, N, N, N, N, N, N, Y, N, N, N, N, N, N, N, Y, N, N, N, N, N, N, N),
    THREAD_SYN_PRINT   -> List(Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, Y, N, N, N, N, N),
    THREAD_SET_BASE    -> List(Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, Y, N, N, N, N),
    THREAD_ERET        -> List(Y, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, N, Y, N, N, N),
  )
}
