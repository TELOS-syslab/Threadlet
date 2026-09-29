// See LICENSE for license details

package roccaccutils.logger

import chisel3._

import org.chipsalliance.cde.config.{Parameters}
import freechips.rocketchip.diplomacy.{ValName}

trait Logger {

  // --------------------------
  // MUST BE DEFINED BY CHILD
  // --------------------------

  def logInfoImplPrintWrapper(printf: chisel3.printf.Printf)(implicit p: Parameters = Parameters.empty): chisel3.printf.Printf
  def logCriticalImplPrintWrapper(printf: chisel3.printf.Printf)(implicit p: Parameters = Parameters.empty): chisel3.printf.Printf

  // --------------------------

  def trimValName()(implicit valName: ValName): String = {
    // TODO: For now don't trim since it can have different pre/post-fixes
    //val trimAmt = if (valName.value.startsWith("<local")) 6 else 1
    //println(s"Got this: ${valName.value}")
    //"<" + valName.value.substring(trimAmt, valName.value.length)
    valName.value


  }

  def createPrefix(typ: String)(implicit valName: ValName, withMod: Boolean, prefix: String): String = {
    val s = Seq(s"${typ}", "%d") ++ (if (withMod) Seq(trimValName()) else Seq.empty) ++ (if (prefix != "") Seq(prefix) else Seq.empty)
    ":" + s.mkString(":") + ": "
  }

  def createFmtAndArgs(typ: String, format: String, args: Bits*)(implicit valName: ValName, withMod: Boolean, prefix: String): (String, Seq[Bits]) = {
    val loginfo_cycles = RegInit(0.U(64.W))
    loginfo_cycles := loginfo_cycles + 1.U

    val allargs = Seq(loginfo_cycles) ++ args
    val allfmt = createPrefix(typ) + format
    (allfmt, allargs)
  }

  def logInfoImpl(format: String, args: Bits*)(implicit p: Parameters = Parameters.empty, valName: ValName, withMod: Boolean, prefix: String): Unit = {
    val (allfmt, allargs) = createFmtAndArgs("INFO", format, args:_*)
    logInfoImplPrintWrapper(printf(Printable.pack(allfmt, allargs:_*)))
  }

  def logCriticalImpl(format: String, args: Bits*)(implicit p: Parameters = Parameters.empty, valName: ValName, withMod: Boolean, prefix: String): Unit = {
    val (allfmt, allargs) = createFmtAndArgs("CRIT", format, args:_*)
    logCriticalImplPrintWrapper(printf(Printable.pack(allfmt, allargs:_*)))
  }

  // ---- USE THE FUNCTIONS BELOW ----

  def logInfo(format: String, args: Bits*)(implicit p: Parameters = Parameters.empty, valName: ValName = ValName("<UnknownMod>"), prefix: String = ""): Unit = {
    implicit val withMod = true
    logInfoImpl(format, args:_*)
  }

  def logCritical(format: String, args: Bits*)(implicit p: Parameters = Parameters.empty, valName: ValName = ValName("<UnknownMod>"), prefix: String = ""): Unit = {
    implicit val withMod = true
    logCriticalImpl(format, args:_*)
  }

  def logInfoNoMod(format: String, args: Bits*)(implicit p: Parameters = Parameters.empty, valName: ValName = ValName("<UnknownMod>"), prefix: String = ""): Unit = {
    implicit val withMod = false
    logInfoImpl(format, args:_*)
  }

  def logCriticalNoMod(format: String, args: Bits*)(implicit p: Parameters = Parameters.empty, valName: ValName = ValName("<UnknownMod>"), prefix: String = ""): Unit = {
    implicit val withMod = false
    logCriticalImpl(format, args:_*)
  }
}

// An example of a custom logger (that optionally only synthesizes critical messages):
//
//   object MyLogger extends Logger {
//     // just print info msgs
//     def logInfoImplPrintWrapper(printf: chisel3.printf.Printf)(implicit p: Parameters = Parameters.empty): chisel3.printf.Printf = {
//       printf
//     }
//
//     // optionally synthesize critical msgs
//     def logCriticalImplPrintWrapper(printf: chisel3.printf.Printf)(implicit p: Parameters = Parameters.empty): chisel3.printf.Printf = {
//       if (p(EnablePrintfSynthesis)) {
//         SynthesizePrintf(printf) // function comes from midas.targetutils
//       } else {
//         printf
//       }
//     }
//   }

object DefaultLogger extends Logger {
  // just print info msgs
  def logInfoImplPrintWrapper(printf: chisel3.printf.Printf)(implicit p: Parameters = Parameters.empty): chisel3.printf.Printf = {
    printf
  }

  // just print critical msgs
  def logCriticalImplPrintWrapper(printf: chisel3.printf.Printf)(implicit p: Parameters = Parameters.empty): chisel3.printf.Printf = {
    printf
  }
}
