// SPDX-License-Identifier: MPL-2.0

use alloc::{collections::btree_map::BTreeMap, string::String, sync::Arc};

use ostd::sync::{LocalIrqDisabled, SpinLock};
use smoltcp::{
    iface::{packet::Packet, Config, Context},
    phy::{Device, DeviceCapabilities, TxToken},
    wire::{
        self, ArpOperation, ArpPacket, ArpRepr, EthernetAddress, EthernetFrame, EthernetProtocol,
        EthernetRepr, IpAddress, Ipv4Address, Ipv4AddressExt, Ipv4Cidr, Ipv4Packet,
    },
};

use crate::{
    device::{NotifyDevice, WithDevice},
    ext::Ext,
    iface::{
        common::IfaceCommon, iface::internal::IfaceInternal, time::get_network_timestamp, Iface,
        ScheduleNextPoll,
    },
};

pub struct EtherIface<D, E: Ext> {
    driver: D,
    common: IfaceCommon<E>,
    ether_addr: EthernetAddress,
    arp_table: SpinLock<BTreeMap<Ipv4Address, EthernetAddress>, LocalIrqDisabled>,
    // Cached IPv4 configuration to avoid locking smoltcp::Interface for simple queries.
    ip_cidr: Ipv4Cidr,
    gateway: Ipv4Address,
}

impl<D: WithDevice, E: Ext> EtherIface<D, E> {
    pub fn new(
        driver: D,
        ether_addr: EthernetAddress,
        ip_cidr: Ipv4Cidr,
        gateway: Ipv4Address,
        name: String,
        sched_poll: E::ScheduleNextPoll,
    ) -> Arc<Self> {
        let interface = driver.with(|device| {
            let config = Config::new(wire::HardwareAddress::Ethernet(ether_addr));
            let now = get_network_timestamp();

            let mut interface = smoltcp::iface::Interface::new(config, device, now);
            interface.update_ip_addrs(|ip_addrs| {
                debug_assert!(ip_addrs.is_empty());
                ip_addrs.push(wire::IpCidr::Ipv4(ip_cidr)).unwrap();
            });
            interface
                .routes_mut()
                .add_default_ipv4_route(gateway)
                .unwrap();
            interface
        });

        let common = IfaceCommon::new(name, interface, sched_poll);

        Arc::new(Self {
            driver,
            common,
            ether_addr,
            arp_table: SpinLock::new(BTreeMap::new()),
            ip_cidr,
            gateway,
        })
    }
}

impl<D, E: Ext> IfaceInternal<E> for EtherIface<D, E> {
    fn common(&self) -> &IfaceCommon<E> {
        &self.common
    }
}

impl<D: WithDevice + 'static, E: Ext> Iface<E> for EtherIface<D, E>
where
    D::Device: NotifyDevice,
{
    fn poll(&self) {
        ostd::arch::riscv::threadlet::threadlet_syn_print(5, 160);
        self.driver.with(|device| {
            // Capture immutable device capabilities before entering poll to
            // avoid borrowing `device` both mutably and immutably.
            let caps = device.capabilities();
            ostd::arch::riscv::threadlet::threadlet_syn_print(5, 162);
            let next_poll = self.common.poll(
                &mut *device,
                |data, tx_token| self.process(data, tx_token),
                |pkt, tx_token| self.dispatch_with_caps(pkt, &caps, tx_token),
            );
            ostd::arch::riscv::threadlet::threadlet_syn_print(5, 163);
            device.notify_poll_end();
            ostd::arch::riscv::threadlet::threadlet_syn_print(5, 164);
            self.common.sched_poll().schedule_next_poll(next_poll);
            ostd::arch::riscv::threadlet::threadlet_syn_print(5, 165);
        });
        #[cfg(target_arch = "riscv64")]
        ostd::arch::riscv::threadlet::threadlet_syn_print(5, 166);
    }
}

impl<D, E: Ext> EtherIface<D, E> {
    fn process<'pkt, T: TxToken>(
        &self,
        data: &'pkt [u8],
        tx_token: T,
    ) -> Option<(Ipv4Packet<&'pkt [u8]>, T)> {
        match self.parse_ip_or_process_arp(data) {
            Ok(pkt) => Some((pkt, tx_token)),
            Err(Some(arp)) => {
                Self::emit_arp(&arp, tx_token);
                None
            }
            Err(None) => None,
        }
    }

    fn parse_ip_or_process_arp<'pkt>(
        &self,
        data: &'pkt [u8],
    ) -> Result<Ipv4Packet<&'pkt [u8]>, Option<ArpRepr>> {
        // Parse the Ethernet header. Ignore the packet if the header is ill-formed.
        let frame = EthernetFrame::new_checked(data).map_err(|_| None)?;
        let repr = EthernetRepr::parse(&frame).map_err(|_| None)?;

        // Ignore the Ethernet frame if it is not sent to us.
        if !repr.dst_addr.is_broadcast() && repr.dst_addr != self.ether_addr {
            return Err(None);
        }

        // Ignore the Ethernet frame if the protocol is not supported.
        match repr.ethertype {
            EthernetProtocol::Ipv4 => {
                Ok(Ipv4Packet::new_checked(frame.payload()).map_err(|_| None)?)
            }
            EthernetProtocol::Arp => {
                let pkt = ArpPacket::new_checked(frame.payload()).map_err(|_| None)?;
                let arp = ArpRepr::parse(&pkt).map_err(|_| None)?;
                Err(self.process_arp(&arp))
            }
            _ => Err(None),
        }
    }

    fn process_arp(&self, arp_repr: &ArpRepr) -> Option<ArpRepr> {
        match arp_repr {
            ArpRepr::EthernetIpv4 {
                operation: ArpOperation::Reply,
                source_hardware_addr,
                source_protocol_addr,
                ..
            } => {
                // Ignore the ARP packet if the source addresses are not unicast or not local.
                if !source_hardware_addr.is_unicast()
                    || !self.in_same_network_local(*source_protocol_addr)
                {
                    return None;
                }

                // Insert the mapping between the Ethernet address and the IP address.
                //

                self.arp_table
                    .lock()
                    .insert(*source_protocol_addr, *source_hardware_addr);

                None
            }
            ArpRepr::EthernetIpv4 {
                operation: ArpOperation::Request,
                source_hardware_addr,
                source_protocol_addr,
                target_protocol_addr,
                ..
            } => {
                // Ignore the ARP packet if the source addresses are not unicast.
                if !source_hardware_addr.is_unicast() || !source_protocol_addr.x_is_unicast() {
                    return None;
                }

                // Ignore the ARP packet if we do not own the target address.
                if self.ipv4_addr() != *target_protocol_addr
                {
                    return None;
                }

                Some(ArpRepr::EthernetIpv4 {
                    operation: ArpOperation::Reply,
                    source_hardware_addr: self.ether_addr,
                    source_protocol_addr: *target_protocol_addr,
                    target_hardware_addr: *source_hardware_addr,
                    target_protocol_addr: *source_protocol_addr,
                })
            }
            _ => None,
        }
    }

    fn dispatch_with_caps<T: TxToken>(&self, pkt: &Packet, caps: &DeviceCapabilities, tx_token: T) {
        match self.resolve_ether_or_generate_arp(pkt) {
            Ok(ether) => Self::emit_ip(&ether, pkt, caps, tx_token),
            Err(Some(arp)) => Self::emit_arp(&arp, tx_token),
            Err(None) => (),
        }
    }

    fn resolve_ether_or_generate_arp(
        &self,
        pkt: &Packet,
    ) -> Result<EthernetRepr, Option<ArpRepr>> {
        // Resolve the next-hop IP address with local route logic (no interface lock).
        let next_hop_ip = match self.route_lookup(&pkt.ip_repr().dst_addr()) {
            Some(next_hop_ip) => next_hop_ip,
            None => return Err(None),
        };

        // Resolve the next-hop Ethernet address.
        let next_hop_ether = if next_hop_ip.is_broadcast() {
            EthernetAddress::BROADCAST
        } else if let Some(next_hop_ether) = self.arp_table.lock().get(&next_hop_ip) {
            *next_hop_ether
        } else {
            // If the next-hop Ethernet address cannot be resolved, we drop the original packet and
            // send an ARP packet instead. The upper layer should be responsible for detecting the
            // packet loss and retrying later to see if the Ethernet address is ready.
            return Err(Some(ArpRepr::EthernetIpv4 {
                operation: ArpOperation::Request,
                source_hardware_addr: self.ether_addr,
                source_protocol_addr: self.ipv4_addr(),
                target_hardware_addr: EthernetAddress::BROADCAST,
                target_protocol_addr: next_hop_ip,
            }));
        };

        Ok(EthernetRepr {
            src_addr: self.ether_addr,
            dst_addr: next_hop_ether,
            ethertype: EthernetProtocol::Ipv4,
        })
    }

    /// Consumes the token and emits an IP packet.
    fn emit_ip<T: TxToken>(
        ether_repr: &EthernetRepr,
        ip_pkt: &Packet,
        caps: &DeviceCapabilities,
        tx_token: T,
    ) {
        tx_token.consume(
            ether_repr.buffer_len() + ip_pkt.ip_repr().buffer_len(),
            |buffer| {
                let mut frame = EthernetFrame::new_unchecked(buffer);
                ether_repr.emit(&mut frame);

                let ip_repr = ip_pkt.ip_repr();
                ip_repr.emit(frame.payload_mut(), &caps.checksum);
                ip_pkt.emit_payload(
                    &ip_repr,
                    &mut frame.payload_mut()[ip_repr.header_len()..],
                    caps,
                );
            },
        );
    }

    /// Consumes the token and emits an ARP packet.
    fn emit_arp<T: TxToken>(arp_repr: &ArpRepr, tx_token: T) {
        let ether_repr = match arp_repr {
            ArpRepr::EthernetIpv4 {
                source_hardware_addr,
                target_hardware_addr,
                ..
            } => EthernetRepr {
                src_addr: *source_hardware_addr,
                dst_addr: *target_hardware_addr,
                ethertype: EthernetProtocol::Arp,
            },
            _ => return,
        };

        tx_token.consume(ether_repr.buffer_len() + arp_repr.buffer_len(), |buffer| {
            let mut frame = EthernetFrame::new_unchecked(buffer);
            ether_repr.emit(&mut frame);

            let mut pkt = ArpPacket::new_unchecked(frame.payload_mut());
            arp_repr.emit(&mut pkt);
        });
    }

    #[inline]
    fn ipv4_addr(&self) -> Ipv4Address { self.ip_cidr.address() }

    #[inline]
    fn in_same_network_local(&self, addr: Ipv4Address) -> bool {
        // Basic CIDR check: (addr & mask) == (local & mask)
        let mask = self.ip_cidr.netmask();
        (addr.to_bits() & mask.to_bits()) == (self.ip_cidr.address().to_bits() & mask.to_bits())
    }

    #[inline]
    fn route_lookup(&self, dst: &IpAddress) -> Option<Ipv4Address> {
        match dst {
            IpAddress::Ipv4(ipv4) => {
                if ipv4.is_broadcast() { return Some(*ipv4); }
                if self.in_same_network_local(*ipv4) { Some(*ipv4) } else { Some(self.gateway) }
            }
        }
    }
}
