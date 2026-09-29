#ifndef __SSHPORT_H
#define __SSHPORT_H

#include <queue>

#include <linux/if.h>
#include <linux/if_tun.h>
#include <sys/ioctl.h>

#define NET_IP_ALIGN 2
#define ETH_MAX_WORDS 190
#define ETH_MAX_BYTES 1518

struct network_flit {
  uint64_t data;
  bool last;
};

/* The other side of this port is a TAP interface to the host network.
 * This allows users to ssh into a simulated cluster */
class SSHPort : public BasePort {
public:
  SSHPort(int portNo);
  void tick();
  void tick_pre();
  void send();
  void recv();

private:
  int sshtapfd;
  uint64_t tap_send_buffer[ETH_MAX_WORDS], tap_recv_buffer[ETH_MAX_WORDS];
  void *tap_send_frame = ((char *)tap_send_buffer) + NET_IP_ALIGN;
  void *tap_recv_frame = ((char *)tap_recv_buffer) + NET_IP_ALIGN;
  int tap_send_idx = 0, tap_len;
  bool tap_can_send = false;
  std::queue<network_flit> out_flits;
  std::queue<network_flit> in_flits;
};

/* open TAP device */
static int tuntap_alloc(const char *dev, int flags) {
  struct ifreq ifr;
  int tapfd, err;

  if ((tapfd = open("/dev/net/tun", O_RDWR | O_NONBLOCK)) < 0) {
    perror("open()");
    return tapfd;
  }

  memset(&ifr, 0, sizeof(ifr));
  ifr.ifr_flags = flags;
  strncpy(ifr.ifr_name, dev, IFNAMSIZ);

  if ((err = ioctl(tapfd, TUNSETIFF, &ifr)) < 0) {
    perror("ioctl()");
    close(tapfd);
    return err;
  }

  return tapfd;
}

#define DEVNAME_BYTES 128
#define ceil_div(n, d) (((n)-1) / (d) + 1)

SSHPort::SSHPort(int portNo) : BasePort(portNo, false) {
  char *slotid =
      NULL; // placeholder for multiple SSH port support if we need it later
  char devname[DEVNAME_BYTES + 1];
  devname[0] = '\0';
  strncat(devname, "tap", DEVNAME_BYTES);

  if (!slotid) {
    fprintf(stderr, "Slot ID not specified. Assuming tap0\n");
    slotid = (char *)"0";
  }
  strncat(devname, slotid, DEVNAME_BYTES - 3);

  sshtapfd = tuntap_alloc(devname, IFF_TAP | IFF_NO_PI);
  if (sshtapfd < 0) {
    fprintf(stderr, "Could not open tap interface %s\n", devname);
    abort();
  }

  current_input_buf = (uint8_t *)calloc(sizeof(uint8_t), BUFSIZE_BYTES);
  current_output_buf = (uint8_t *)calloc(sizeof(uint8_t), BUFSIZE_BYTES);
}




// static inline int real_eth_len(const uint8_t* f, int guessed_len) {
//   if (guessed_len < 14) return guessed_len;

//   auto be16 = [](const uint8_t* p)->int { return (int)(p[0] << 8 | p[1]); };

//   int off = 12;
//   int et  = be16(f + off);

//   if (et == 0x8100 || et == 0x88A8) {
//     if (guessed_len < 18) return guessed_len;
//     off = 16;
//     et  = be16(f + off);
//   }

//   if (et == 0x0806) return 42;

//   if (et == 0x0800) {
//     if (guessed_len < off + 4) return guessed_len;
//     const uint8_t* ip = f + 14 + ((off==16)?4:0); 
//     if (guessed_len < (ip + 4 - f)) return guessed_len;
//     int ip_tot = be16(ip + 2);
//     int ihl = (ip[0] & 0x0F) * 4;
//     if (ip_tot < ihl || ip_tot > 65535) return guessed_len;
//     return 14 + ((off==16)?4:0) + ip_tot;
//   }

//   if (et == 0x86DD) {
//     const uint8_t* ip6 = f + 14 + ((off==16)?4:0);
//     if (guessed_len < (ip6 + 6 - f)) return guessed_len;
//     int payload = be16(ip6 + 4);
//     return 14 + ((off==16)?4:0) + 40 + payload;
//   }
//   return guessed_len;
// }

// void SSHPort::send() {
//   if (((uint64_t *)current_output_buf)[0] == 0xDEADBEEFDEADBEEFL) {
//     ((uint64_t *)current_output_buf)[0] = 0L;
//   }

//   for (int tokenno = 0; tokenno < NUM_TOKENS; tokenno++) {
//     if (is_valid_flit(current_output_buf, tokenno)) {
//       struct network_flit flt;
//       flt.data = get_flit(current_output_buf, tokenno);
//       flt.last = is_last_flit(current_output_buf, tokenno);
//       out_flits.push(flt);
//     }
//   }

//   if (!tap_can_send) {
//     while (!out_flits.empty()) {
//       tap_send_buffer[tap_send_idx] = out_flits.front().data;
//       tap_can_send = out_flits.front().last;
//       out_flits.pop();
//       tap_send_idx++;
//       if (tap_can_send) break;
//     }
//   }

//   if (tap_can_send) {
//     int guessed = tap_send_idx * (int)sizeof(uint64_t) - NET_IP_ALIGN;
//     if (guessed < 14) guessed = 14;               
//     if (guessed > ETH_MAX_BYTES) guessed = ETH_MAX_BYTES;

//     const uint8_t* frame = (const uint8_t*)tap_send_frame;
//     int true_len = real_eth_len(frame, guessed);
//     if (true_len > guessed) true_len = guessed;     
//     if (true_len < 14) true_len = 14;

//     if (::write(sshtapfd, tap_send_frame, true_len) >= 0) {
//       tap_send_idx = 0;
//       tap_can_send = false;
//     } else if (errno != EAGAIN) {
//       perror("send()");
//       abort();
//     }
//   }
//   memset(current_output_buf, 0x0, BUFSIZE_BYTES);
// }



void SSHPort::send() {
  // here, we take data that was written to the port by the switch
  // (data is in current_output_buf)
  // and push it into queues to send into the TAP

  if (((uint64_t *)current_output_buf)[0] == 0xDEADBEEFDEADBEEFL) {
    // if compress flag is set, clear it, this port type doesn't care
    // (and in fact, we're writing too much, so stuff later will get confused)
    ((uint64_t *)current_output_buf)[0] = 0L;
  }

  // first, push into out_flits queue
  for (int tokenno = 0; tokenno < NUM_TOKENS; tokenno++) {
    if (is_valid_flit(current_output_buf, tokenno)) {
      struct network_flit flt;
      flt.data = get_flit(current_output_buf, tokenno);
      flt.last = is_last_flit(current_output_buf, tokenno);
      out_flits.push(flt);
    }
  }

  // then, actually send stuff out on the TAP
  // next, see if there is data to send
  if (!tap_can_send) {
    while (!out_flits.empty()) {
      tap_send_buffer[tap_send_idx] = out_flits.front().data;
      tap_can_send = out_flits.front().last;
      out_flits.pop();
      tap_send_idx++;
      if (tap_can_send)
        break;
    }
  }

  if (tap_can_send) {
    tap_len = tap_send_idx * sizeof(uint64_t) - NET_IP_ALIGN;
    if (::write(sshtapfd, tap_send_frame, tap_len) >= 0) {
      tap_send_idx = 0;
      tap_can_send = false;
    } else if (errno != EAGAIN) {
      perror("send()");
      abort();
    }
  }

  // finally, clear current_output_buf for the next iter
  memset(current_output_buf, 0x0, BUFSIZE_BYTES);
}



void SSHPort::recv() {
  // clear the input buf leftover from previous cycle
  memset(current_input_buf, 0x0, BUFSIZE_BYTES);

  // pull in flits from the TAP
  tap_len = ::read(sshtapfd, tap_recv_frame, ETH_MAX_BYTES);
  if (tap_len >= 0) {
    int i, n = ceil_div(tap_len + NET_IP_ALIGN, sizeof(uint64_t));
    for (i = 0; i < n; i++) {
      struct network_flit flt;
      flt.data = tap_recv_buffer[i];
      flt.last = i == (n - 1);
      in_flits.push(flt);
    }
  } else if (errno != EAGAIN) {
    perror("recv()");
    abort();
  }

  // next, pull off of in_flits until current_input_buf is full, or we have
  // nothing left to write

  for (int tokenno = 0; tokenno < NUM_TOKENS; tokenno++) {
    if (!in_flits.empty()) {
      write_last_flit(current_input_buf, tokenno, in_flits.front().last);
      write_valid_flit(current_input_buf, tokenno);
      write_flit(current_input_buf, tokenno, in_flits.front().data);
      in_flits.pop();
    }
  }
}

void SSHPort::tick() {
  // don't need to do anything for SSHPorts
}

void SSHPort::tick_pre() {
  // don't need to do anything for SSHPorts
}
#endif // __SSHPORT_H
