// SPDX-License-Identifier: MPL-2.0

#include <arpa/inet.h>
#include <errno.h>
#include <netinet/in.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>

int main(void) {
    int sockfd = socket(AF_INET, SOCK_DGRAM, 0);
    if (sockfd < 0) {
        perror("udp_wait: socket");
        return 1;
    }

    int opt = 1;
    (void)setsockopt(sockfd, SOL_SOCKET, SO_REUSEADDR, &opt, sizeof(opt));

    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    // Bind to 0.0.0.0:5555 so hostfwd udp::11181->:5555 reaches us
    addr.sin_addr.s_addr = htonl(INADDR_ANY);
    addr.sin_port = htons(5555);

    if (bind(sockfd, (struct sockaddr *)&addr, sizeof(addr)) < 0) {
        perror("udp_wait: bind 0.0.0.0:5555");
        close(sockfd);
        return 1;
    }

    printf("udp_wait: started (listening on 0.0.0.0:5555)\n");
    fflush(stdout);

    char buf[1024];
    struct sockaddr_in src;
    socklen_t srclen = sizeof(src);
    ssize_t n = recvfrom(sockfd, buf, sizeof(buf) - 1, 0, (struct sockaddr *)&src, &srclen);
    if (n < 0) {
        perror("udp_wait: recvfrom");
        close(sockfd);
        return 1;
    }
    buf[n] = '\0';
    printf("thread I/O wakeup: recv %zd bytes from %s:%d payload='%s'\n",
           n, inet_ntoa(src.sin_addr), ntohs(src.sin_port), buf);
    fflush(stdout);

    close(sockfd);
    return 0;
}

