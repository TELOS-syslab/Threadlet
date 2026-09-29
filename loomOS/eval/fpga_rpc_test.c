// SPDX-License-Identifier: MPL-2.0

#define _GNU_SOURCE
#include <arpa/inet.h>
#include <errno.h>
#include <linux/if_packet.h>
#include <net/ethernet.h>
#include <net/if.h>
#include <netinet/ether.h>
#include <netinet/ip.h>
#include <netinet/udp.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <unistd.h>
#include <endian.h>


#ifndef ETH_ALEN
#define ETH_ALEN 6
#endif

#define RPC_MAGIC 0x7777ULL
#define DEFAULT_MAX_RPC 10000
#define DEFAULT_DPORT 5555
#define KEY_SIZE 10
#define VALUE_SIZE 100
#define DEFAULT_SCAN_FREQ 200
#define DEFAULT_ENABLE_WAIT 0
#define DEFAULT_WAIT_RUN_NS 200ULL
#define DEFAULT_WAIT_LONG_MULTIPLIER 10ULL
#define DEFAULT_SEND_SLEEP_US 50U
#define DEFAULT_NUM_WARMUP 0
#define DEFAULT_DISPATCH_FANOUT 1U

enum RpcType {
    Put = 0,    
    Get = 1,
    Finish = 2,
    Scan = 3,
    Wait = 4,
};

typedef struct Rpc {
    uint64_t magic;
    uint32_t id;
    uint32_t type;
    char key[KEY_SIZE];
    char value[VALUE_SIZE];
} Rpc;

typedef struct RpcWait {
    uint64_t magic;
    uint32_t id;
    uint32_t type;
    char key[KEY_SIZE];
    char value[VALUE_SIZE];
    uint64_t run_ns;
} __attribute__((packed)) RpcWait;

static void die(const char *msg) {
    perror(msg);
    exit(1);
}

static unsigned short csum16(const void *buf, size_t len) {
    // Internet checksum (RFC 1071).
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
    while (sum >> 16) {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    return (unsigned short)(~sum);
}

static int parse_mac(const char *s, unsigned char mac[6]) {
    struct ether_addr *ea = ether_aton(s);
    if (!ea) {
        return -1;
    }
    memcpy(mac, ea->ether_addr_octet, 6);
    return 0;
}

int main(int argc, char **argv) {
    const char *ifname = NULL;
    unsigned char dst_mac[6] = {0x00, 0x12, 0x6D, 0x00, 0x00, 0x02};
    const char *dst_ip_str = "172.16.0.2";
    const char *src_ip_str = "172.16.0.1";
    unsigned short dport = DEFAULT_DPORT;
    int max_rpc = DEFAULT_MAX_RPC;
    int scan_freq = DEFAULT_SCAN_FREQ;
    int enable_wait = DEFAULT_ENABLE_WAIT;
    uint64_t wait_run_ns = DEFAULT_WAIT_RUN_NS;
    uint64_t wait_long_multiplier = DEFAULT_WAIT_LONG_MULTIPLIER;
    unsigned int send_sleep_us = DEFAULT_SEND_SLEEP_US;
    int num_warmup = DEFAULT_NUM_WARMUP;
    unsigned int dispatch_fanout = DEFAULT_DISPATCH_FANOUT;

    int opt;
    while ((opt = getopt(argc, argv, "i:d:D:S:p:n:f:w:r:l:s:u:k:h")) != -1) {
        switch (opt) {
        case 'i':
            ifname = optarg;
            break;
        case 'd':
            if (parse_mac(optarg, dst_mac) != 0) {
                fprintf(stderr, "Invalid MAC: %s\n", optarg);
                return 2;
            }
            break;
        case 'D':
            dst_ip_str = optarg;
            break;
        case 'S':
            src_ip_str = optarg;
            break;
        case 'p':
            dport = (unsigned short)strtoul(optarg, NULL, 10);
            break;
        case 'n':
            max_rpc = atoi(optarg);
            break;
        case 'f':
            scan_freq = atoi(optarg);
            break;
        case 'w':
            enable_wait = atoi(optarg);
            break;
        case 'r':
            wait_run_ns = strtoull(optarg, NULL, 10);
            break;
        case 'l':
            wait_long_multiplier = strtoull(optarg, NULL, 10);
            break;
        case 's':
            send_sleep_us = (unsigned int)strtoul(optarg, NULL, 10);
            break;
        case 'u':
            num_warmup = atoi(optarg);
            break;
        case 'k':
            dispatch_fanout = (unsigned int)strtoul(optarg, NULL, 10);
            break;
        case 'h':
        default:
            fprintf(
                stderr,
                "Usage: %s -i <iface> -d <dst-mac> -D <dst-ip> [-S <src-ip>] [-p <dst-port>] [-n <max-rpc>] [-f <scan-freq>] [-w <enable-wait:0|1>] [-r <wait-short-ns>] [-l <wait-long-multiplier>] [-s <send-sleep-us>] [-u <num-warmup>] [-k <dispatch-fanout:1-9>]\n",
                argv[0]
            );
            return 2;
        }
    }

    if (!ifname) {
        fprintf(stderr, "Interface is required.\n");
        return 2;
    }
    if (max_rpc <= 0) {
        fprintf(stderr, "Invalid max_rpc=%d, fallback to %d\n", max_rpc, DEFAULT_MAX_RPC);
        max_rpc = DEFAULT_MAX_RPC;
    }
    if (scan_freq <= 0) {
        fprintf(stderr, "Invalid scan_freq=%d, fallback to %d\n", scan_freq, DEFAULT_SCAN_FREQ);
        scan_freq = DEFAULT_SCAN_FREQ;
    }
    if (!(enable_wait == 0 || enable_wait == 1)) {
        fprintf(stderr, "Invalid enable_wait=%d, must be 0 or 1.\n", enable_wait);
        return 2;
    }
    if (wait_long_multiplier == 0) {
        fprintf(stderr, "Invalid wait_long_multiplier=0, must be > 0.\n");
        return 2;
    }
    if (wait_run_ns > (UINT64_MAX / wait_long_multiplier)) {
        fprintf(stderr, "Invalid wait_run_ns=%llu: overflow for long wait.\n", (unsigned long long)wait_run_ns);
        return 2;
    }
    if (num_warmup < 0) {
        fprintf(stderr, "Invalid num_warmup=%d, must be >= 0.\n", num_warmup);
        return 2;
    }
    if (dispatch_fanout == 0 || dispatch_fanout >= 10) {
        fprintf(stderr, "Invalid dispatch_fanout=%u, must be in [1, 9].\n", dispatch_fanout);
        return 2;
    }

    int fd = socket(AF_PACKET, SOCK_RAW, htons(ETH_P_ALL));
    if (fd < 0) {
        die("socket(AF_PACKET)");
    }

    struct ifreq ifr;
    memset(&ifr, 0, sizeof(ifr));
    strncpy(ifr.ifr_name, ifname, IFNAMSIZ - 1);
    if (ioctl(fd, SIOCGIFINDEX, &ifr) < 0) {
        die("ioctl(SIOCGIFINDEX)");
    }
    int ifindex = ifr.ifr_ifindex;

    if (ioctl(fd, SIOCGIFHWADDR, &ifr) < 0) {
        die("ioctl(SIOCGIFHWADDR)");
    }
    unsigned char src_mac[6];
    memcpy(src_mac, ifr.ifr_hwaddr.sa_data, 6);

    Rpc rpc;
    memset(&rpc, 0, sizeof(rpc));
    rpc.magic = htobe64(RPC_MAGIC);
    strncpy(rpc.key, "user", KEY_SIZE - 1);
    strncpy(rpc.value, "xxxxxxxxxxxx", VALUE_SIZE - 1);
    uint32_t dispatch_fanout_be = htobe32(dispatch_fanout);
    memcpy(rpc.value, &dispatch_fanout_be, sizeof(dispatch_fanout_be));

    RpcWait rpc_wait;
    memset(&rpc_wait, 0, sizeof(rpc_wait));
    rpc_wait.magic = htobe64(RPC_MAGIC);
    strncpy(rpc_wait.key, "user", KEY_SIZE - 1);
    strncpy(rpc_wait.value, "xxxxxxxxxxxx", VALUE_SIZE - 1);
    memcpy(rpc_wait.value, &dispatch_fanout_be, sizeof(dispatch_fanout_be));
    rpc_wait.run_ns = htobe64(wait_run_ns);

    unsigned char frame[1514];
    size_t off = 0;

    struct ether_header *eth = (struct ether_header *)(frame + off);
    memcpy(eth->ether_dhost, dst_mac, 6);
    memcpy(eth->ether_shost, src_mac, 6);
    eth->ether_type = htons(ETH_P_IP);
    off += sizeof(struct ether_header);

    struct iphdr *ip = (struct iphdr *)(frame + off);
    memset(ip, 0, sizeof(*ip));
    ip->version = 4;
    ip->ihl = 5;
    ip->tos = 0;
    ip->id = htons(0x1234);
    ip->frag_off = 0;
    ip->ttl = 64;
    ip->protocol = IPPROTO_UDP;
    if (inet_pton(AF_INET, src_ip_str, &ip->saddr) != 1) {
        die("inet_pton(src)");
    }
    if (inet_pton(AF_INET, dst_ip_str, &ip->daddr) != 1) {
        die("inet_pton(dst)");
    }
    off += sizeof(struct iphdr);

    struct udphdr *udp = (struct udphdr *)(frame + off);
    memset(udp, 0, sizeof(*udp));
    udp->source = htons(54321);
    udp->dest = htons(dport);
    off += sizeof(struct udphdr);

    unsigned char *payload_ptr = frame + off;
    size_t payload_len = enable_wait ? sizeof(RpcWait) : sizeof(Rpc);
    memset(payload_ptr, 0, payload_len);
    off += payload_len;

    size_t udp_len = sizeof(struct udphdr) + payload_len;
    size_t ip_len = sizeof(struct iphdr) + udp_len;
    ip->tot_len = htons(ip_len);
    udp->len = htons(udp_len);

    ip->check = 0;
    ip->check = csum16(ip, sizeof(struct iphdr));

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

    size_t cksz = sizeof(pseudo) + udp_len;
    unsigned char *ckbuf = malloc(cksz);
    if (!ckbuf) {
        die("malloc");
    }

    struct sockaddr_ll sll;
    memset(&sll, 0, sizeof(sll));
    sll.sll_family = AF_PACKET;
    sll.sll_ifindex = ifindex;
    sll.sll_halen = ETH_ALEN;
    memcpy(sll.sll_addr, dst_mac, ETH_ALEN);

    size_t frame_len = off;
    size_t frame_count = (size_t)max_rpc;
    if (frame_len != 0 && frame_count > (SIZE_MAX / frame_len)) {
        free(ckbuf);
        fprintf(stderr, "frame buffer overflow: frame_count=%zu frame_len=%zu\n", frame_count, frame_len);
        close(fd);
        return 2;
    }
    unsigned char *frame_buf = malloc(frame_count * frame_len);
    if (!frame_buf) {
        free(ckbuf);
        die("malloc(frame_buf)");
    }

    // Prebuild all packets before the first send to minimize inter-packet send gap.
    for (int i = 0; i < max_rpc; i++) {
        unsigned char *pkt = frame_buf + ((size_t)i * frame_len);
        memcpy(pkt, frame, frame_len);

        struct udphdr *pkt_udp = (struct udphdr *)(pkt + sizeof(struct ether_header) + sizeof(struct iphdr));
        unsigned char *pkt_payload = pkt + sizeof(struct ether_header) + sizeof(struct iphdr) + sizeof(struct udphdr);

        uint32_t type = enable_wait ? Wait : ((i % scan_freq == 0) ? Scan : Get);
        if (i == max_rpc) {
            type = Finish;
        }

        if (enable_wait) {
            uint64_t run_ns_this_req = 0;
            if (type == Wait) {
                int is_long_wait = (i >= num_warmup) &&
                    (i % scan_freq == 0);
                run_ns_this_req = is_long_wait
                    ? (wait_run_ns * wait_long_multiplier)
                    : wait_run_ns;
            }
            rpc_wait.id = htobe32((uint32_t)i);
            rpc_wait.type = htobe32((uint32_t)type);
            rpc_wait.run_ns = htobe64(run_ns_this_req);
            memcpy(pkt_payload, &rpc_wait, sizeof(rpc_wait));
        } else {
            rpc.id = htobe32((uint32_t)i);
            rpc.type = htobe32((uint32_t)type);
            memcpy(pkt_payload, &rpc, sizeof(rpc));
        }

        pkt_udp->check = 0;
        memcpy(ckbuf, &pseudo, sizeof(pseudo));
        memcpy(ckbuf + sizeof(pseudo), pkt_udp, sizeof(struct udphdr));
        memcpy(ckbuf + sizeof(pseudo) + sizeof(struct udphdr), pkt_payload, payload_len);
        unsigned short sum = csum16(ckbuf, cksz);
        if (sum == 0) {
            sum = 0xFFFF; // RFC768: checksum of zero is sent as all-ones.
        }
        pkt_udp->check = sum;
    }

    ssize_t n = 0;
    for (int i = 0; i <= max_rpc; i++) {
        if (i == max_rpc) {
            // Send FINISH one second after all normal requests are sent.
           // sleep(1);
           break; 
        } 
        usleep(send_sleep_us);

        unsigned char *pkt = frame_buf + ((size_t)i * frame_len);
        n = sendto(fd, pkt, frame_len, 0, (struct sockaddr *)&sll, sizeof(sll));
        if (n < 0) {
            free(frame_buf);
            free(ckbuf);
            die("sendto");
        }
    }

    free(frame_buf);
    free(ckbuf);
    printf(
        "Sent RPC packets(%zu-byte payload each), dst=%02x:%02x:%02x:%02x:%02x:%02x %s:%u magic=0x%llx\n",
        payload_len,
        dst_mac[0],
        dst_mac[1],
        dst_mac[2],
        dst_mac[3],
        dst_mac[4],
        dst_mac[5],
        dst_ip_str,
        dport,
        (unsigned long long)RPC_MAGIC
    );

    close(fd);
    return 0;
}
