// SPDX-License-Identifier: MPL-2.0

use alloc::vec;

use aster_bigtcp::{
    device::{self, NotifyDevice},
    time::Instant,
};
use ostd::mm::VmWriter;

use crate::{buffer::RxBuffer, AnyNetworkDevice};

impl device::Device for dyn AnyNetworkDevice {
    type RxToken<'a> = RxToken;
    type TxToken<'a> = TxToken<'a>;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        // Debug: observe gating conditions right before popping RX
        let can_rx = self.can_receive();
        let can_tx = self.can_send();
        if can_rx && can_tx {
            // Pop one RX buffer from the driver
            let rx_buffer = self.receive().unwrap();
            Some((RxToken(rx_buffer), TxToken(self)))
        } else {
            None
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        if self.can_send() {
            Some(TxToken(self))
        } else {
            None
        }
    }

    fn capabilities(&self) -> device::DeviceCapabilities {
        self.capabilities()
    }
}

impl NotifyDevice for dyn AnyNetworkDevice {
    fn notify_poll_end(&mut self) {
        self.notify_poll_end();
    }
}

pub struct RxToken(RxBuffer);

impl device::RxToken for RxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        let mut packet = self.0.packet();
        let mut buffer = vec![0u8; packet.remain()];
        packet.read(&mut VmWriter::from(&mut buffer as &mut [u8]));
        #[cfg(irqdebug)]
        {
            // Trace: show RX path and first few bytes (ASCII preview)
            let show = core::cmp::min(64, buffer.len());
            let mut ascii_buf = [0u8; 64];
            for i in 0..show {
                let b = buffer[i];
                ascii_buf[i] = if (0x20..=0x7e).contains(&b) { b } else { b'.' };
            }
            let ascii = core::str::from_utf8(&ascii_buf[..show]).unwrap_or("");
            ostd::early_println!(
                "[rx] frame: len={} head-ascii='{}'",
                buffer.len(),
                ascii
            );

            // Best-effort parse Ethernet/IPv4/(UDP|TCP) to locate payload and show ASCII
            let mut printed_payload = false;
            if buffer.len() >= 14 {
                let ethertype = u16::from_be_bytes([buffer[12], buffer[13]]);
                if ethertype == 0x0800 /* IPv4 */ && buffer.len() >= 14 + 20 {
                    let ip_start = 14;
                    let ihl = (buffer[ip_start] & 0x0f) as usize * 4;
                    if ihl >= 20 && buffer.len() >= ip_start + ihl {
                        let protocol = buffer[ip_start + 9];
                        // IPv4 addresses
                        let src_ip = [
                            buffer[ip_start + 12],
                            buffer[ip_start + 13],
                            buffer[ip_start + 14],
                            buffer[ip_start + 15],
                        ];
                        let dst_ip = [
                            buffer[ip_start + 16],
                            buffer[ip_start + 17],
                            buffer[ip_start + 18],
                            buffer[ip_start + 19],
                        ];
                        // total length from header
                        let total_len = u16::from_be_bytes([
                            buffer[ip_start + 2],
                            buffer[ip_start + 3],
                        ]) as usize;
                        let payload_base = ip_start + ihl;
                        if protocol == 17 /* UDP */ && buffer.len() >= ip_start + ihl + 8 {
                            let udp_start = payload_base;
                            let src_port = u16::from_be_bytes([buffer[udp_start], buffer[udp_start + 1]]);
                            let dst_port = u16::from_be_bytes([buffer[udp_start + 2], buffer[udp_start + 3]]);
                            let udp_len = u16::from_be_bytes([buffer[udp_start + 4], buffer[udp_start + 5]]) as usize;
                            let payload_off = udp_start + 8;
                            if buffer.len() >= payload_off {
                                let payload_len = udp_len.saturating_sub(8).min(buffer.len() - payload_off);
                                let preview = core::cmp::min(64, payload_len);
                                let mut pbuf = [0u8; 64];
                                for i in 0..preview {
                                    let b = buffer[payload_off + i];
                                    pbuf[i] = if (0x20..=0x7e).contains(&b) { b } else { b'.' };
                                }
                                let pascii = core::str::from_utf8(&pbuf[..preview]).unwrap_or("");
                                ostd::early_println!(
                                    "[rx] ipv4/udp 5-tuple: {}.{}.{}.{}:{} -> {}.{}.{}.{}:{} payload_len={} ascii='{}'",
                                    src_ip[0], src_ip[1], src_ip[2], src_ip[3], src_port,
                                    dst_ip[0], dst_ip[1], dst_ip[2], dst_ip[3], dst_port,
                                    payload_len,
                                    pascii
                                );
                                printed_payload = true;
                            }
                        } else if protocol == 6 /* TCP */ && buffer.len() >= ip_start + ihl + 20 {
                            let tcp_start = payload_base;
                            let src_port = u16::from_be_bytes([buffer[tcp_start], buffer[tcp_start + 1]]);
                            let dst_port = u16::from_be_bytes([buffer[tcp_start + 2], buffer[tcp_start + 3]]);
                            let data_off = ((buffer[tcp_start + 12] >> 4) as usize) * 4;
                            let payload_off = tcp_start + data_off;
                            if buffer.len() >= payload_off {
                                let payload_len = buffer.len().saturating_sub(payload_off);
                                let preview = core::cmp::min(64, payload_len);
                                let mut pbuf = [0u8; 64];
                                for i in 0..preview {
                                    let b = buffer[payload_off + i];
                                    pbuf[i] = if (0x20..=0x7e).contains(&b) { b } else { b'.' };
                                }
                                let pascii = core::str::from_utf8(&pbuf[..preview]).unwrap_or("");
                                ostd::early_println!(
                                    "[rx] ipv4/tcp 5-tuple: {}.{}.{}.{}:{} -> {}.{}.{}.{}:{} payload_len={} ascii='{}'",
                                    src_ip[0], src_ip[1], src_ip[2], src_ip[3], src_port,
                                    dst_ip[0], dst_ip[1], dst_ip[2], dst_ip[3], dst_port,
                                    payload_len,
                                    pascii
                                );
                                printed_payload = true;
                            }
                        } else {
                            // Not UDP/TCP: print IPv4 header summary (protocol, totlen)
                            ostd::early_println!(
                                "[rx] ipv4: proto={} totlen={} src={}.{}.{}.{} dst={}.{}.{}.{}",
                                protocol, total_len,
                                src_ip[0], src_ip[1], src_ip[2], src_ip[3],
                                dst_ip[0], dst_ip[1], dst_ip[2], dst_ip[3]
                            );
                            // ICMP details if present
                            if protocol == 1 /* ICMP */ && buffer.len() >= payload_base + 2 {
                                let icmp_type = buffer[payload_base];
                                let icmp_code = buffer[payload_base + 1];
                                ostd::early_println!(
                                    "[rx] icmp: type={} code={} (no UDP/TCP payload)",
                                    icmp_type, icmp_code
                                );
                            }
                        }
                    }
                }
                else if ethertype == 0x0806 /* ARP */ {
                    // Parse ARP (best-effort)
                    let arp_start = 14;
                    if buffer.len() >= arp_start + 8 {
                        let hlen = buffer[arp_start + 4] as usize;
                        let plen = buffer[arp_start + 5] as usize;
                        let oper = u16::from_be_bytes([buffer[arp_start + 6], buffer[arp_start + 7]]);
                        let addr_base = arp_start + 8;
                        let need = addr_base + 2 * hlen + 2 * plen;
                        if buffer.len() >= need {
                            // Only print for IPv4/ethernet typical case
                            let sha = &buffer[addr_base..addr_base + hlen.min(6)];
                            let spa = &buffer[addr_base + hlen..addr_base + hlen + plen.min(4)];
                            let tha_off = addr_base + hlen + plen;
                            let tha = &buffer[tha_off..tha_off + hlen.min(6)];
                            let tpa = &buffer[tha_off + hlen..tha_off + hlen + plen.min(4)];
                            ostd::early_println!(
                                "[rx] arp: op={} sha={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} spa={}.{}.{}.{} tha={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} tpa={}.{}.{}.{}",
                                oper,
                                sha.get(0).copied().unwrap_or(0), sha.get(1).copied().unwrap_or(0), sha.get(2).copied().unwrap_or(0),
                                sha.get(3).copied().unwrap_or(0), sha.get(4).copied().unwrap_or(0), sha.get(5).copied().unwrap_or(0),
                                spa.get(0).copied().unwrap_or(0), spa.get(1).copied().unwrap_or(0), spa.get(2).copied().unwrap_or(0), spa.get(3).copied().unwrap_or(0),
                                tha.get(0).copied().unwrap_or(0), tha.get(1).copied().unwrap_or(0), tha.get(2).copied().unwrap_or(0),
                                tha.get(3).copied().unwrap_or(0), tha.get(4).copied().unwrap_or(0), tha.get(5).copied().unwrap_or(0),
                                tpa.get(0).copied().unwrap_or(0), tpa.get(1).copied().unwrap_or(0), tpa.get(2).copied().unwrap_or(0), tpa.get(3).copied().unwrap_or(0)
                            );
                            printed_payload = true; // we printed meaningful info
                        }
                    }
                } else {
                    // Unknown ethertype
                    ostd::early_println!("[rx] ethertype=0x{:04x} (not IPv4/ARP)", ethertype);
                }
            }
            if !printed_payload {
                ostd::early_println!("[rx] payload: (no UDP/TCP payload detected)");
            }
        }
        f(&buffer)
    }
}

pub struct TxToken<'a>(&'a mut dyn AnyNetworkDevice);

impl device::TxToken for TxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        // Ethernet may requires a minimum frame length of 64 bytes on the wire
        const ETH_MIN_NBYTES_NO_FCS: usize = 60;
        let pad_len = core::cmp::max(len, ETH_MIN_NBYTES_NO_FCS);
        let mut buffer = vec![0u8; pad_len];
        // Let upper layer fill the actual packet data of `len` bytes.
        let res = f(&mut buffer[..len]);
        // The tail [len..pad_len] stays zero as padding.
        #[cfg(netdebug)]
        {
            if buffer.len() >= 14 {
                let dst = &buffer[0..6];
                let src = &buffer[6..12];
                let ethertype = u16::from_be_bytes([buffer[12], buffer[13]]);
                ostd::early_println!(
                    "[tx] frame: len={} (orig={}) ethertype=0x{:04x} dst={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} src={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    buffer.len(), len, ethertype,
                    dst[0], dst[1], dst[2], dst[3], dst[4], dst[5],
                    src[0], src[1], src[2], src[3], src[4], src[5]
                );
                if ethertype == 0x0806 /* ARP */ && buffer.len() >= 42 {
                    let arp = 14usize;
                    let oper = u16::from_be_bytes([buffer[arp + 6], buffer[arp + 7]]);
                    let sha = &buffer[arp + 8..arp + 14];
                    let spa = &buffer[arp + 14..arp + 18];
                    let tha = &buffer[arp + 18..arp + 24];
                    let tpa = &buffer[arp + 24..arp + 28];
                    ostd::early_println!(
                        "[tx] arp: op={} sha={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} spa={}.{}.{}.{} tha={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} tpa={}.{}.{}.{}",
                        oper,
                        sha[0], sha[1], sha[2], sha[3], sha[4], sha[5],
                        spa[0], spa[1], spa[2], spa[3],
                        tha[0], tha[1], tha[2], tha[3], tha[4], tha[5],
                        tpa[0], tpa[1], tpa[2], tpa[3]
                    );
                }
            }
        }
        self.0.send(&buffer).expect("Send packet failed");
        res
    }
}
