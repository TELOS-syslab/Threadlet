// SPDX-License-Identifier: MPL-2.0

use alloc::{string::String, sync::Arc};

use smoltcp::{
    iface::Config,
    phy::{Device, TxToken},
    wire::{self, Ipv4Cidr, Ipv4Packet},
};

use crate::{
    device::WithDevice,
    ext::Ext,
    iface::{
        common::IfaceCommon, iface::internal::IfaceInternal, time::get_network_timestamp, Iface,
        ScheduleNextPoll,
    },
};

pub struct IpIface<D, E: Ext> {
    driver: D,
    common: IfaceCommon<E>,
}

impl<D: WithDevice, E: Ext> IpIface<D, E> {
    pub fn new(
        driver: D,
        ip_cidr: Ipv4Cidr,
        name: String,
        sched_poll: E::ScheduleNextPoll,
    ) -> Arc<Self> {
        let interface = driver.with(|device| {
            let config = Config::new(smoltcp::wire::HardwareAddress::Ip);
            let now = get_network_timestamp();

            let mut interface = smoltcp::iface::Interface::new(config, device, now);
            interface.update_ip_addrs(|ip_addrs| {
                debug_assert!(ip_addrs.is_empty());
                ip_addrs.push(wire::IpCidr::Ipv4(ip_cidr)).unwrap();
            });
            interface
        });

        let common = IfaceCommon::new(name, interface, sched_poll);

        Arc::new(Self { driver, common })
    }
}

impl<D, E: Ext> IfaceInternal<E> for IpIface<D, E> {
    fn common(&self) -> &IfaceCommon<E> {
        &self.common
    }
}

impl<D: WithDevice + 'static, E: Ext> Iface<E> for IpIface<D, E> {
    fn poll(&self) {
        self.driver.with(|device| {
            // Capture immutable device capabilities before entering poll to
            // avoid aliasing borrows of `device` inside the callbacks.
            let caps = device.capabilities();
            let next_poll = self.common.poll(
                device,
                |data, tx_token| Some((Ipv4Packet::new_checked(data).ok()?, tx_token)),
                |pkt, tx_token| {
                    let ip_repr = pkt.ip_repr();
                    tx_token.consume(ip_repr.buffer_len(), |buffer| {
                        ip_repr.emit(&mut buffer[..], &caps.checksum);
                        pkt.emit_payload(
                            &ip_repr,
                            &mut buffer[ip_repr.header_len()..],
                            &caps,
                        );
                    });
                },
            );
            self.common.sched_poll().schedule_next_poll(next_poll);
        });
    }
}
