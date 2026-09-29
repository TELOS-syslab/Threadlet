// SPDX-License-Identifier: MPL-2.0

#define _GNU_SOURCE
#include <arpa/inet.h>
#include <ctype.h>
#include <errno.h>
#include <linux/if_packet.h>
#include <net/ethernet.h>
#include <net/if.h>
#include <netinet/ether.h>
#include <netinet/ip.h>
#include <netinet/udp.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <unistd.h>

#ifndef ETH_ALEN
#define ETH_ALEN 6
#endif

static void die(const char *msg) {
    perror(msg);
    exit(1);
}

static unsigned short csum16(const void *buf, size_t len) {
    // Internet checksum (RFC 1071)
    const unsigned short *data = buf;
    unsigned long sum = 0;
    while (len > 1) {
        sum += *data++;
        len -= 2;
    }
    if (len == 1) {
        unsigned short last = 0;
        *(unsigned char *)&last = *(const unsigned char *)data;
        sum += last;
    }
    while (sum >> 16) sum = (sum & 0xFFFF) + (sum >> 16);
    return (unsigned short)(~sum);
}

static int parse_mac(const char *s, unsigned char mac[6]) {
    struct ether_addr *ea = ether_aton(s);
    if (!ea) return -1;
    memcpy(mac, ea->ether_addr_octet, 6);
    return 0;
}

int main(int argc, char **argv) {
    const char *ifname = NULL;
    unsigned char dst_mac[6] = {0x00,0x12,0x6D,0x00,0x00,0x02};
    const char *dst_ip_str = "10.0.2.15";
    const char *src_ip_str = "10.0.2.2";
    unsigned short dport = 10081;
    const char *payload = "hello-icenet";

    int opt;
    while ((opt = getopt(argc, argv, "i:d:D:S:p:s:h")) != -1) {
        switch (opt) {
        case 'i': ifname = optarg; break;
        case 'd': if (parse_mac(optarg, dst_mac) != 0) { fprintf(stderr, "Invalid MAC: %s\n", optarg); return 2; } break;
        case 'D': dst_ip_str = optarg; break;
        case 'S': src_ip_str = optarg; break;
        case 'p': dport = (unsigned short)strtoul(optarg, NULL, 10); break;
        case 's': payload = optarg; break;
        case 'h':
        default:
            fprintf(stderr, "Usage: %s -i <iface> -d <dst-mac> -D <dst-ip> [-S <src-ip>] [-p <dst-port>] [-s <payload>]\n", argv[0]);
            return 2;
        }
    }
    if (!ifname) {
        fprintf(stderr, "Interface is required.\n");
        return 2;
    }

    // Resolve interface index and source MAC
    int fd = socket(AF_PACKET, SOCK_RAW, htons(ETH_P_ALL));
    if (fd < 0) die("socket(AF_PACKET)");

    struct ifreq ifr;
    memset(&ifr, 0, sizeof(ifr));
    strncpy(ifr.ifr_name, ifname, IFNAMSIZ - 1);
    if (ioctl(fd, SIOCGIFINDEX, &ifr) < 0) die("ioctl(SIOCGIFINDEX)");
    int ifindex = ifr.ifr_ifindex;

    if (ioctl(fd, SIOCGIFHWADDR, &ifr) < 0) die("ioctl(SIOCGIFHWADDR)");
    unsigned char src_mac[6];
    memcpy(src_mac, ifr.ifr_hwaddr.sa_data, 6);

    // Build packet buffers: ETH + IP + UDP + payload
    unsigned char frame[1514];
    size_t off = 0;

    // Ethernet header
    struct ether_header *eth = (struct ether_header *)(frame + off);
    memcpy(eth->ether_dhost, dst_mac, 6);
    memcpy(eth->ether_shost, src_mac, 6);
    eth->ether_type = htons(ETH_P_IP);
    off += sizeof(struct ether_header);

    // IP header
    struct iphdr *ip = (struct iphdr *)(frame + off);
    memset(ip, 0, sizeof(*ip));
    ip->version = 4;
    ip->ihl = 5;
    ip->tos = 0;
    // length filled later
    ip->id = htons(0x1234);
    ip->frag_off = 0;
    ip->ttl = 64;
    ip->protocol = IPPROTO_UDP;
    if (inet_pton(AF_INET, src_ip_str, &ip->saddr) != 1) die("inet_pton(src)");
    if (inet_pton(AF_INET, dst_ip_str, &ip->daddr) != 1) die("inet_pton(dst)");
    off += sizeof(struct iphdr);

    // UDP header
    struct udphdr *udp = (struct udphdr *)(frame + off);
    memset(udp, 0, sizeof(*udp));
    udp->source = htons(54321);
    udp->dest = htons(dport);
    off += sizeof(struct udphdr);

    // Payload
    size_t payload_len = strlen(payload);
    memcpy(frame + off, payload, payload_len);
    off += payload_len;

    // Fill lengths
    size_t udp_len = sizeof(struct udphdr) + payload_len;
    size_t ip_len = sizeof(struct iphdr) + udp_len;
    ip->tot_len = htons(ip_len);
    udp->len = htons(udp_len);

    // Compute checksums: IP header and UDP with pseudo-header
    ip->check = 0;
    ip->check = csum16(ip, sizeof(struct iphdr));

    // UDP checksum (pseudo-header)
    struct {
        uint32_t saddr;
        uint32_t daddr;
        uint8_t zero;
        uint8_t proto;
        uint16_t len;
    } __attribute__((packed)) pseudo;
    pseudo.saddr = ip->saddr;
    pseudo.daddr = ip->daddr;
    pseudo.zero = 0;
    pseudo.proto = IPPROTO_UDP;
    pseudo.len = udp->len;

    // Build contiguous buffer for checksum
    size_t cksz = sizeof(pseudo) + udp_len;
    unsigned char *ckbuf = malloc(cksz);
    if (!ckbuf) die("malloc");
    memcpy(ckbuf, &pseudo, sizeof(pseudo));
    memcpy(ckbuf + sizeof(pseudo), udp, sizeof(struct udphdr));
    memcpy(ckbuf + sizeof(pseudo) + sizeof(struct udphdr), frame + sizeof(struct ether_header) + sizeof(struct iphdr) + sizeof(struct udphdr), payload_len);
    udp->check = 0;
    unsigned short sum = csum16(ckbuf, cksz);
    free(ckbuf);
    if (sum == 0) sum = 0xFFFF; // RFC768: checksum of zero transmitted as all-ones
    udp->check = sum;

    // Send via AF_PACKET
    struct sockaddr_ll sll;
    memset(&sll, 0, sizeof(sll));
    sll.sll_family = AF_PACKET;
    sll.sll_ifindex = ifindex;
    sll.sll_halen = ETH_ALEN;
    memcpy(sll.sll_addr, dst_mac, ETH_ALEN);

    ssize_t n = sendto(fd, frame, off, 0, (struct sockaddr *)&sll, sizeof(sll));
    if (n < 0) die("sendto");
    printf("Sent %zd bytes via %s to %02x:%02x:%02x:%02x:%02x:%02x %s:%u\n",
           n, ifname,
           dst_mac[0], dst_mac[1], dst_mac[2], dst_mac[3], dst_mac[4], dst_mac[5],
           dst_ip_str, dport);

    close(fd);
    return 0;
}

