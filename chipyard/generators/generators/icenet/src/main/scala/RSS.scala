package icenet

import chisel3._
import chisel3.util._
import freechips.rocketchip.util._
import IceNetConsts._
import scala.math.max

class HashIO(nCores: Int) extends Bundle {
  val in = Flipped(Valid(new Bundle {
    val src_ip   = Input(UInt(32.W)) 
    val dst_ip   = Input(UInt(32.W)) 
    val src_port = Input(UInt(32.W))
    val dst_port = Input(UInt(32.W))
    val protocol = Input(UInt(8.W))
  }))

  val out = Valid(UInt(log2Ceil(nCores).W))
}

/*
 * ToeplitzHash same as Intel DPDK
 *
 * @hashBits Width of hash result(in bits)
 * @entires Entries of indirection table
 * @nCores Number of cores
 */
class Hash(hashBits: Int = 32, entries: Int = 16, nCores: Int) extends Module {
  val io = IO(new HashIO(nCores))

  private val rss_key = VecInit(Seq(
    0x6d.U(8.W), 0x5a.U(8.W), 0x56.U(8.W), 0xda.U(8.W),
    0x25.U(8.W), 0x5b.U(8.W), 0x0e.U(8.W), 0xc2.U(8.W),
    0x41.U(8.W), 0x67.U(8.W), 0x25.U(8.W), 0x3d.U(8.W),
    0x43.U(8.W), 0xa3.U(8.W), 0x8f.U(8.W), 0xb0.U(8.W),
    0xd0.U(8.W), 0xca.U(8.W), 0x2b.U(8.W), 0xcb.U(8.W),
    0xae.U(8.W), 0x7b.U(8.W), 0x30.U(8.W), 0xb4.U(8.W),
    0x77.U(8.W), 0xcb.U(8.W), 0x2d.U(8.W), 0xa3.U(8.W),
    0x80.U(8.W), 0x30.U(8.W), 0xf2.U(8.W), 0x0c.U(8.W),
    0x6a.U(8.W), 0x42.U(8.W), 0xb7.U(8.W), 0x3b.U(8.W),
    0xbe.U(8.W), 0xac.U(8.W), 0x01.U(8.W), 0xfa.U(8.W)
  )).asUInt

  private val rss_len: Int = rss_key.widthOption.get

  def ToeplitzHash(array: UInt) = {
    require(array.widthOption.get > 0)
    (0 until array.widthOption.get).foldLeft(0.U(hashBits.W)) { (p, k) => 
      p ^ Mux (((array >> k) & 1.U) === 0.U, 0.U, 
        rss_key(rss_len - k - 1, rss_len - k - hashBits - 1))
    }
  }

  val hash_array = Cat(io.in.bits.src_ip, io.in.bits.dst_ip, 
    io.in.bits.src_port, io.in.bits.dst_port, io.in.bits.protocol)
  val hash_num = ToeplitzHash(hash_array)
  io.out.valid := io.in.valid
  io.out.bits := hash_num(log2Ceil(entries) - 1, 0)
}

/**
 * identify IP address and port number of packets
 * then implement hash function
 * @nCores Number of cores
 */
class RSS(nCores: Int, random: Boolean = false) extends Module 
  with NetworkEndianHelpers {
  val io = IO(new Bundle {
    val in = Flipped(Decoupled(new StreamChannel(NET_IF_WIDTH)))
    // selected core to send packets to
    val hash_core = Valid(UInt(log2Ceil(nCores).W))
  })
  io.in.ready := true.B
   
  class FullTCPHeader extends Bundle {
    val tcp = new TCPHeader
    val ipv4 = new IPv4Header
    val eth = new EthernetHeader
  }
  class FullUDPHeader extends Bundle {
    val align = UInt((TCP_HEAD_BYTES - UDP_HEAD_BYTES).W)
    val udp = new UDPHeader
    val ipv4 = new IPv4Header
    val eth = new EthernetHeader
  }

  val dataBytes = NET_IF_WIDTH / 8

  // save tcp header only
  val tcpHeaderBytes = ETH_HEAD_BYTES + IPV4_HEAD_BYTES + TCP_HEAD_BYTES
  val tcpHeaderWords = tcpHeaderBytes / dataBytes
  val tcpHeaderSlots = Reg(Vec(tcpHeaderWords, UInt(NET_IF_WIDTH.W)))
  val tcpHeader = tcpHeaderSlots.asTypeOf(new FullTCPHeader)

  // save udp header only
  val udpAlign = TCP_HEAD_BYTES - UDP_HEAD_BYTES
  val udpHeaderBytes = ETH_HEAD_BYTES + IPV4_HEAD_BYTES + UDP_HEAD_BYTES + udpAlign
  val udpHeaderWords = udpHeaderBytes / dataBytes
  val udpHeaderSlots = Reg(Vec(udpHeaderWords, UInt(NET_IF_WIDTH.W)))
  val udpHeader = udpHeaderSlots.asTypeOf(new FullUDPHeader)

  val headerWords = max(tcpHeaderWords, udpHeaderWords)
  val headerIdx = RegInit(0.U(log2Ceil(headerWords).W))

  require(tcpHeaderBytes % dataBytes == 0)
  require(udpHeaderBytes % dataBytes == 0)

  val (s_header_in :: s_passthru :: Nil) = Enum(2)
  val state = RegInit(s_header_in)

  // Track packet boundary and ensure at most one hash per packet
  val inPacket = RegInit(false.B)
  val hashSent = RegInit(false.B)

  /*
   * identify IP address and port number of packets
   */

  val isIP = random.B || tcpHeader.eth.ethType === IPV4_ETHTYPE.U
  val isTCP = random.B || (isIP && tcpHeader.ipv4.protocol === TCP_PROTOCOL.U && tcpHeader.ipv4.ihl === 5.U)
  val isUDP = random.B || (isIP && udpHeader.ipv4.protocol === UDP_PROTOCOL.U && udpHeader.ipv4.ihl === 5.U)
  val protocol = Mux(isIP, tcpHeader.ipv4.protocol, 0.U)
  val src_ip = Mux(isIP, tcpHeader.ipv4.source_ip, 0.U)
  val dst_ip = Mux(isIP, tcpHeader.ipv4.dest_ip, 0.U)
  val src_port = Mux(isTCP, tcpHeader.tcp.source_port, Mux(isUDP, udpHeader.udp.source_port, 0.U))
  val dst_port = Mux(isTCP, tcpHeader.tcp.dest_port, Mux(isUDP, udpHeader.udp.dest_port, 0.U))

  /*
   * State Machine
   */

  when (io.in.fire) {
    when (!inPacket) {
      // First beat of a new packet
      inPacket := true.B
      hashSent := false.B
    }

    when (io.in.bits.last) {
      // End of packet: reset state for next packet
      inPacket := false.B
      state := s_header_in
      headerIdx := 0.U
    } .elsewhen (state === s_header_in) {
      // Still collecting header words
      when (headerIdx < tcpHeaderWords.U) {
        tcpHeaderSlots(headerIdx) := io.in.bits.data
      }
      when (headerIdx < udpHeaderWords.U) {
        udpHeaderSlots(headerIdx) := io.in.bits.data
      }
      headerIdx := headerIdx + 1.U
      when (headerIdx === (headerWords - 1).U) {
        state := s_passthru
      }
    }
  }

  val hash = Module(new Hash(hashBits = 32, nCores = nCores))
  hash.io.in.bits.src_ip := src_ip
  hash.io.in.bits.dst_ip := dst_ip
  hash.io.in.bits.src_port := src_port
  hash.io.in.bits.dst_port := dst_port
  hash.io.in.bits.protocol := protocol

  // Generate exactly one hash per packet:
  //  - For long packets, after we have collected full header (state == s_passthru).
  //  - For short packets, on the last beat while still in header_in.
  val longHeaderDone  = (state === s_passthru) && inPacket && !hashSent
  val shortHeaderDone = io.in.fire && io.in.bits.last && (state === s_header_in) && !hashSent
  val doHash          = longHeaderDone || shortHeaderDone

  hash.io.in.valid := doHash

  when (doHash) {
    hashSent := true.B
  }

  io.hash_core.valid := hash.io.out.valid
  io.hash_core.bits := hash.io.out.bits

}
