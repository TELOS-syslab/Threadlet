// SPDX-License-Identifier: MPL-2.0

use alloc::{sync::Arc, vec, vec::Vec};

use smoltcp::{
    iface::{
        packet::{icmp_reply_payload_len, IpPayload, Packet},
        Context,
    },
    phy::{ChecksumCapabilities, Device, RxToken, TxToken},
    wire::{
        Icmpv4DstUnreachable, Icmpv4Repr, IpAddress, IpProtocol, IpRepr, Ipv4Address, Ipv4Packet,
        Ipv4Repr, TcpControl, TcpPacket, TcpRepr, UdpPacket, UdpRepr, IPV4_HEADER_LEN,
        IPV4_MIN_MTU,
    },
};

use crate::{
    ext::Ext,
    socket::{TcpConnectionBg, TcpProcessResult, UdpSocketBg},
    socket_table::{ConnectionKey, ListenerKey, SocketTable},
};
use ostd::sync::{LocalIrqDisabled, SpinLock};
use super::common::CxAccess;

pub(super) struct PollContext<'a, E: Ext> {
    cx: &'a CxAccess<'a, E>,
    now: smoltcp::time::Instant,
    caps: &'a ChecksumCapabilities,
    sockets: &'a SocketTable<E>,
    new_tcp_conns: &'a mut Vec<Arc<TcpConnectionBg<E>>>,
}

impl<'a, E: Ext> PollContext<'a, E> {
    pub(super) fn new(
        cx: &'a CxAccess<'a, E>,
        now: smoltcp::time::Instant,
        caps: &'a ChecksumCapabilities,
        sockets: &'a SocketTable<E>,
        new_tcp_conns: &'a mut Vec<Arc<TcpConnectionBg<E>>>,
    ) -> Self {
        Self {
            cx,
            now,
            caps,
            sockets,
            new_tcp_conns,
        }
    }
}

// This works around <https://github.com/rust-lang/rust/issues/49601>.
// Simplified to 2-arg closure variant used by ingress/egress helpers.
pub(super) trait FnHelper<A, B, O>: FnMut(A, B) -> O {}
impl<A, B, O, F> FnHelper<A, B, O> for F where F: FnMut(A, B) -> O {}

impl<E: Ext> PollContext<'_, E> {
    pub(super) fn poll_ingress<D, P, Q>(
        &mut self,
        device: &mut D,
        process_phy: &mut P,
        dispatch_phy: &mut Q,
    ) where
        D: Device + ?Sized,
        P: for<'pkt, 'tx> FnHelper<
            &'pkt [u8],
            D::TxToken<'tx>,
            Option<(Ipv4Packet<&'pkt [u8]>, D::TxToken<'tx>)>,
        >,
        Q: FnMut(&Packet, D::TxToken<'_>),
    {
        let udp_sockets = self.sockets.udp_sockets_snapshot();
        while let Some((rx_token, tx_token)) = device.receive(self.now) {
            rx_token.consume(|data| {
                // Perform PHY parsing without &mut Context to avoid interface-wide locks.
                let Some((pkt, tx_token)) = process_phy(data, tx_token) else { return; };
                let Some(reply) = self.parse_and_process_ipv4(pkt, &udp_sockets) else { return; };
                // Route/ARP resolution is handled outside smoltcp context.
                dispatch_phy(&reply, tx_token);
            });
        }
    }

    fn parse_and_process_ipv4<'pkt>(
        &mut self,
        pkt: Ipv4Packet<&'pkt [u8]>,
        udp_sockets: &[Arc<UdpSocketBg<E>>],
    ) -> Option<Packet<'pkt>> {
        // Parse the IP header. Ignore the packet if the header is ill-formed.
        let repr = Ipv4Repr::parse(&pkt, self.caps).ok()?;

        if !repr.dst_addr.is_broadcast() && !self.is_unicast_local(IpAddress::Ipv4(repr.dst_addr)) {
            return self.generate_icmp_unreachable(
                &IpRepr::Ipv4(repr),
                pkt.payload(),
                Icmpv4DstUnreachable::HostUnreachable,
            );
        }

        match repr.next_header {
            IpProtocol::Tcp => {
                let caps = self.caps;
                self.parse_and_process_tcp(&IpRepr::Ipv4(repr), pkt.payload(), caps)
            }
            IpProtocol::Udp => {
                let caps = self.caps;
                self.parse_and_process_udp(&IpRepr::Ipv4(repr), pkt.payload(), caps, udp_sockets)
            }
            _ => None,
        }
    }

    fn parse_and_process_tcp<'pkt>(
        &mut self,
        ip_repr: &IpRepr,
        ip_payload: &'pkt [u8],
        checksum_caps: &ChecksumCapabilities,
    ) -> Option<Packet<'pkt>> {
        // TCP connections can only be established between unicast addresses. Ignore the packet if
        // this is not the case. See
        // <https://datatracker.ietf.org/doc/html/rfc9293#section-3.9.2.3>.
        if !ip_repr.src_addr().is_unicast() || !ip_repr.dst_addr().is_unicast() {
            return None;
        }

        // Parse the TCP header. Ignore the packet if the header is ill-formed.
        let tcp_pkt = TcpPacket::new_checked(ip_payload).ok()?;
        let tcp_repr = TcpRepr::parse(
            &tcp_pkt,
            &ip_repr.src_addr(),
            &ip_repr.dst_addr(),
            checksum_caps,
        )
        .ok()?;

        self.process_tcp_until_outgoing(ip_repr, &tcp_repr)
            .map(|(ip_repr, tcp_repr)| Packet::new(ip_repr, IpPayload::Tcp(tcp_repr)))
    }

    fn process_tcp_until_outgoing(
        &mut self,
        ip_repr: &IpRepr,
        tcp_repr: &TcpRepr,
    ) -> Option<(IpRepr, TcpRepr<'static>)> {
        let (mut ip_repr, mut tcp_repr) = self.process_tcp(ip_repr, tcp_repr)?;

        loop {
            if !self.is_unicast_local(ip_repr.dst_addr()) {
                return Some((ip_repr, tcp_repr));
            }

            let (new_ip_repr, new_tcp_repr) = self.process_tcp(&ip_repr, &tcp_repr)?;
            ip_repr = new_ip_repr;
            tcp_repr = new_tcp_repr;
        }
    }

    fn process_tcp(
        &mut self,
        ip_repr: &IpRepr,
        tcp_repr: &TcpRepr,
    ) -> Option<(IpRepr, TcpRepr<'static>)> {
        // Process packets that request to create new connections first.
        if tcp_repr.control == TcpControl::Syn && tcp_repr.ack_number.is_none() {
            let listener_key = ListenerKey::new(ip_repr.dst_addr(), tcp_repr.dst_port);
            let listener = self.sockets.lookup_listener_arc(&listener_key);
            if let Some(listener) = listener {
                let (processed, new_tcp_conn) = self.cx.with(|cx| listener.process(cx, ip_repr, tcp_repr));

                if let Some(tcp_conn) = new_tcp_conn {
                    self.new_tcp_conns.push(tcp_conn);
                }

                match processed {
                    TcpProcessResult::NotProcessed => {}
                    TcpProcessResult::Processed => return None,
                    TcpProcessResult::ProcessedWithReply(ip_repr, tcp_repr) => {
                        return Some((ip_repr, tcp_repr))
                    }
                }
            }
        }

        // Process packets belonging to existing connections second.
        let connection_key = ConnectionKey::new(
            ip_repr.dst_addr(),
            tcp_repr.dst_port,
            ip_repr.src_addr(),
            tcp_repr.src_port,
        );
        let connection = self
            .sockets
            .lookup_connection_arc(&connection_key)
        .or_else(|| {
            self.new_tcp_conns
                .iter()
                .find(|tcp_conn| tcp_conn.connection_key() == &connection_key)
                .cloned()
        });

        if let Some(connection) = connection {
            match self.cx.with(|cx| connection.process(cx, ip_repr, tcp_repr)) {
                TcpProcessResult::NotProcessed => {}
                TcpProcessResult::Processed => return None,
                TcpProcessResult::ProcessedWithReply(ip_repr, tcp_repr) => {
                    return Some((ip_repr, tcp_repr))
                }
            }
        }

        // "In no case does receipt of a segment containing RST give rise to a RST in response."
        // See <https://datatracker.ietf.org/doc/html/rfc9293#section-4-1.64>.
        if tcp_repr.control == TcpControl::Rst {
            return None;
        }

        Some(smoltcp::socket::tcp::Socket::rst_reply(ip_repr, tcp_repr))
    }

    fn parse_and_process_udp<'pkt>(
        &mut self,
        ip_repr: &IpRepr,
        ip_payload: &'pkt [u8],
        checksum_caps: &ChecksumCapabilities,
        udp_sockets: &[Arc<UdpSocketBg<E>>],
    ) -> Option<Packet<'pkt>> {
        // Parse the UDP header. Ignore the packet if the header is ill-formed.
        let udp_pkt = UdpPacket::new_checked(ip_payload).ok()?;
        let udp_repr = UdpRepr::parse(
            &udp_pkt,
            &ip_repr.src_addr(),
            &ip_repr.dst_addr(),
            checksum_caps,
        )
        .ok()?;

        if !self.process_udp(ip_repr, &udp_repr, udp_pkt.payload(), udp_sockets) {
            return self.generate_icmp_unreachable(
                ip_repr,
                ip_payload,
                Icmpv4DstUnreachable::PortUnreachable,
            );
        }

        None
    }

    fn process_udp(
        &mut self,
        ip_repr: &IpRepr,
        udp_repr: &UdpRepr,
        udp_payload: &[u8],
        udp_sockets: &[Arc<UdpSocketBg<E>>],
    ) -> bool {
        let mut processed = false;

        for socket in udp_sockets.iter() {
            if !socket.can_process(udp_repr.dst_port) {
                continue;
            }

            processed |= self.cx.with(|cx| socket.process(cx, ip_repr, udp_repr, udp_payload));
            if processed && ip_repr.dst_addr().is_unicast() {
                break;
            }
        }

        processed
    }

    fn generate_icmp_unreachable<'pkt>(
        &self,
        ip_repr: &IpRepr,
        ip_payload: &'pkt [u8],
        reason: Icmpv4DstUnreachable,
    ) -> Option<Packet<'pkt>> {
        if !ip_repr.src_addr().is_unicast() || !ip_repr.dst_addr().is_unicast() {
            return None;
        }

        if self.is_unicast_local(ip_repr.src_addr()) {
            // In this case, the generating ICMP message will have a local IP address as the
            // destination. However, since we don't have the ability to handle ICMP messages, we'll
            // just skip the generation.
            //

            // messages.
            return None;
        }

        let IpRepr::Ipv4(ipv4_repr) = ip_repr;

        let reply_len = icmp_reply_payload_len(ip_payload.len(), IPV4_MIN_MTU, IPV4_HEADER_LEN);
        let icmp_repr = Icmpv4Repr::DstUnreachable {
            reason,
            header: *ipv4_repr,
            data: &ip_payload[..reply_len],
        };

        Some(Packet::new_ipv4(
            Ipv4Repr {
                src_addr: self
                    .cx
                    .ipv4_addr()
                    .unwrap_or(Ipv4Address::UNSPECIFIED),
                dst_addr: ipv4_repr.src_addr,
                next_header: IpProtocol::Icmp,
                payload_len: icmp_repr.buffer_len(),
                hop_limit: 64,
            },
            IpPayload::Icmpv4(icmp_repr),
        ))
    }

    /// Returns whether the destination address is the unicast address of a local interface.
    ///
    /// Note: "local" means that the IP address belongs to the local interface, not to be confused
    /// with the localhost IP (127.0.0.1).
    fn is_unicast_local(&self, dst_addr: IpAddress) -> bool {
        match dst_addr {
            IpAddress::Ipv4(dst_addr) => self
                .cx
                .ipv4_addr()
                .is_some_and(|addr| addr == dst_addr),
        }
    }
}

impl<E: Ext> PollContext<'_, E> {
    pub(super) fn poll_egress<D, Q>(&mut self, device: &mut D, dispatch_phy: &mut Q)
    where
        D: Device + ?Sized,
        Q: FnMut(&Packet, D::TxToken<'_>),
    {
        let tcp_conns = self.sockets.tcp_connections_snapshot();
        let udp_sockets = self.sockets.udp_sockets_snapshot();
        while let Some(tx_token) = device.transmit(self.now) {
            if !self.dispatch_ipv4(tx_token, dispatch_phy, &tcp_conns, &udp_sockets) {
                break;
            }
        }
    }

    fn dispatch_ipv4<T, Q>(
        &mut self,
        tx_token: T,
        dispatch_phy: &mut Q,
        tcp_conns: &[Arc<TcpConnectionBg<E>>],
        udp_sockets: &[Arc<UdpSocketBg<E>>],
    ) -> bool
    where
        T: TxToken,
        Q: FnMut(&Packet, T),
    {
        let (did_something_tcp, tx_token) = self.dispatch_tcp(tx_token, dispatch_phy, tcp_conns);

        let Some(tx_token) = tx_token else {
            return did_something_tcp;
        };

        let (did_something_udp, _tx_token) =
            self.dispatch_udp(tx_token, dispatch_phy, udp_sockets);

        did_something_tcp || did_something_udp
    }

    fn dispatch_tcp<T, Q>(
        &mut self,
        tx_token: T,
        dispatch_phy: &mut Q,
        tcp_conns: &[Arc<TcpConnectionBg<E>>],
    ) -> (bool, Option<T>)
    where
        T: TxToken,
        Q: FnMut(&Packet, T),
    {
        let mut tx_token = Some(tx_token);
        let mut did_something = false;

        // We cannot dispatch packets from `new_tcp_conns` because we cannot borrow an immutable
        // reference at this point. Instead, we will retry after the entire poll is complete.
        for socket in tcp_conns.iter() {
            if !socket.need_dispatch(self.now) {
                continue;
            }

            // We set `did_something` even if no packets are actually generated. This is because a
            // timer can expire, but no packets are actually generated.
            did_something = true;

            let mut deferred: Option<(IpRepr, Vec<u8>)> = None;

            let reply = self.cx.with(|cx| {
                TcpConnectionBg::dispatch(socket, cx, |cx, ip_repr, tcp_repr| {
                    // Check locality using cx
                    let is_local = match ip_repr.dst_addr() {
                        IpAddress::Ipv4(addr) => cx.ipv4_addr().is_some_and(|a| a == addr),
                    };
                    if !is_local {
                        if let Some(tok) = tx_token.take() {
                            dispatch_phy(&Packet::new(ip_repr.clone(), IpPayload::Tcp(*tcp_repr)), tok);
                        }
                        return None;
                    }

                    if !socket.can_process(tcp_repr.dst_port) {
                        // Defer local processing outside the socket lock by encoding the TCP repr.
                        deferred = Some((ip_repr.clone(), {
                            let mut data = vec![0; tcp_repr.buffer_len()];
                            tcp_repr.emit(
                                &mut TcpPacket::new_unchecked(data.as_mut_slice()),
                                &ip_repr.src_addr(),
                                &ip_repr.dst_addr(),
                                &ChecksumCapabilities::ignored(),
                            );
                            data
                        }));
                        return None;
                    }

                    // Defer local processing to avoid deadlocks
                    deferred = Some((ip_repr.clone(), {
                        let mut data = vec![0; tcp_repr.buffer_len()];
                        tcp_repr.emit(
                            &mut TcpPacket::new_unchecked(data.as_mut_slice()),
                            &ip_repr.src_addr(),
                            &ip_repr.dst_addr(),
                            &ChecksumCapabilities::ignored(),
                        );
                        data
                    }));
                    None
                })
            });

            match (deferred, reply) {
                (None, None) => (),
                (Some((ip_repr, ip_payload)), None) => {
                    if let Some(reply) =
                        self.parse_and_process_tcp(&ip_repr, &ip_payload, &ChecksumCapabilities::ignored())
                    {
                        if let Some(tok) = tx_token.take() { dispatch_phy(&reply, tok); }
                    }
                }
                (None, Some((ip_repr, tcp_repr))) if !self.is_unicast_local(ip_repr.dst_addr()) => {
                    if let Some(tok) = tx_token.take() { dispatch_phy(&Packet::new(ip_repr, IpPayload::Tcp(tcp_repr)), tok); }
                }
                (None, Some((ip_repr, tcp_repr))) => {
                    if let Some((new_ip_repr, new_tcp_repr)) =
                        self.process_tcp_until_outgoing(&ip_repr, &tcp_repr)
                    {
                        if let Some(tok) = tx_token.take() { dispatch_phy(&Packet::new(new_ip_repr, IpPayload::Tcp(new_tcp_repr)), tok); }
                    }
                }
                (Some(_), Some(_)) => unreachable!(),
            }

            if tx_token.is_none() {
                break;
            }
        }

        (did_something, tx_token)
    }

    fn dispatch_udp<T, Q>(
        &mut self,
        tx_token: T,
        dispatch_phy: &mut Q,
        udp_sockets: &[Arc<UdpSocketBg<E>>],
    ) -> (bool, Option<T>)
    where
        T: TxToken,
        Q: FnMut(&Packet, T),
    {
        let mut tx_token = Some(tx_token);
        let mut did_something = false;

        for socket in udp_sockets.iter() {
            if !socket.need_dispatch(self.now) {
                continue;
            }

            // We set `did_something` even if no packets are actually generated. This is because a
            // timer can expire, but no packets are actually generated.
            did_something = true;

        let mut deferred: Option<(IpRepr, Vec<u8>)> = None;
        let mut need_local_udp: bool = false;
        let mut saved_ip: Option<IpRepr> = None;
        let mut saved_udp: Option<UdpRepr> = None;
        let mut saved_payload: Option<Vec<u8>> = None;

            self.cx.with(|cx| {
                socket.dispatch(cx, |cx, ip_repr, udp_repr, udp_payload| {
                    let is_local = match ip_repr.dst_addr() {
                        IpAddress::Ipv4(addr) => cx.ipv4_addr().is_some_and(|a| a == addr),
                    };

                    if ip_repr.dst_addr().is_broadcast() || !is_local {
                        if let Some(tok) = tx_token.take() { dispatch_phy(&Packet::new(ip_repr.clone(), IpPayload::Udp(*udp_repr, udp_payload)), tok); }
                        if !ip_repr.dst_addr().is_broadcast() { return; }
                    }

                    if !socket.can_process(udp_repr.dst_port) {
                        need_local_udp = true;
                        saved_ip = Some(ip_repr.clone());
                        saved_udp = Some(*udp_repr);
                        saved_payload = Some(udp_payload.to_vec());
                        return;
                    }

                    // Defer local processing
                    deferred = Some((ip_repr.clone(), {
                        let mut data = vec![0; udp_repr.header_len() + udp_payload.len()];
                        udp_repr.emit(
                            &mut UdpPacket::new_unchecked(&mut data),
                            &ip_repr.src_addr(),
                            &ip_repr.dst_addr(),
                            udp_payload.len(),
                            |payload| payload.copy_from_slice(udp_payload),
                            &ChecksumCapabilities::ignored(),
                        );
                        data
                    }));
                });
            });

            if need_local_udp {
                if let (Some(ip), Some(udp), Some(payload)) = (saved_ip.take(), saved_udp.take(), saved_payload.take()) {
                    let _ = self.process_udp(&ip, &udp, &payload, udp_sockets);
                }
            }

            if let Some((ip_repr, ip_payload)) = deferred {
                if let Some(reply) = self.parse_and_process_udp(
                    &ip_repr,
                    &ip_payload,
                    &ChecksumCapabilities::ignored(),
                    udp_sockets,
                ) {
                    if let Some(tok) = tx_token.take() { dispatch_phy(&reply, tok); }
                }
            }

            if tx_token.is_none() {
                break;
            }
        }

        (did_something, tx_token)
    }
}
