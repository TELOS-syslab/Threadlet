#include <memory>
#include <string>
#include <vector>
#include <chrono>
#include <random>
#include <stdlib.h>
#include <cstring>
#include <sys/socket.h>
#include <arpa/inet.h>
#include <unistd.h>
#include <time.h>
#include <errno.h>
#include <algorithm>

#define OP_STORE 1
#define OP_GET   2

#define SERVER_IP "172.16.0.2" 
#define SERVER_PORT 8888

typedef struct {
    int operation;
    int key_len;
    int val_len;
} RpcRequestHeader;

typedef struct {
    int status;
    int val_len;
} RpcResponseHeader;


std::string random_string(size_t length) {
    auto randchar = []() -> char {
        const char charset[] =
        "0123456789"
        "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
        "abcdefghijklmnopqrstuvwxyz";
        const size_t max_index = (sizeof(charset) - 1);
        return charset[rand() % max_index];
    };
    std::string str(length, 0);
    std::generate_n(str.begin(), length, randchar);
    return str;
}

void measure_rpc_performance(int sock, int op_type, const char* key_str, const char* val_str) {
    size_t key_len = strlen(key_str);
    size_t val_len = (val_str == NULL) ? 0 : strlen(val_str);
    size_t data_payload_len = key_len + val_len;
    size_t total_request_len = sizeof(RpcRequestHeader) + data_payload_len;

    char* request_buffer = (char*)malloc(total_request_len);
    if (!request_buffer) {
        perror("malloc failed");
        return;
    }

    RpcRequestHeader header = {
        .operation = op_type,
        .key_len = (int)key_len,
        .val_len = (int)val_len
    };

    memcpy(request_buffer, &header, sizeof(RpcRequestHeader));
    memcpy(request_buffer + sizeof(RpcRequestHeader), key_str, key_len);
    if (val_len > 0) {
        memcpy(request_buffer + sizeof(RpcRequestHeader) + key_len, val_str, val_len);
    }

    int num_requests = 10000;
    long long total_time_ns = 0;
    char response_buffer[2048];

    printf("Sending %d %s requests (Key:%zuB, Val:%zuB) ---\n", 
           num_requests, (op_type == OP_STORE ? "STORE" : "GET"), key_len, val_len);

    for (int i = 0; i < num_requests; i++) {
        struct timespec start, end;
        
        clock_gettime(CLOCK_MONOTONIC, &start);

        if (send(sock, request_buffer, total_request_len, 0) < 0) {
            perror("send failed");
            break;
        }

        ssize_t received = recv(sock, response_buffer, sizeof(RpcResponseHeader), MSG_WAITALL);
        if (received <= 0) {
            fprintf(stderr, "recv failed or connection closed (i=%d). Error: %s\n", i, strerror(errno));
            break;
        }

        RpcResponseHeader* resp_header = (RpcResponseHeader*)response_buffer;

        if (resp_header->val_len > 0) {
            received = recv(sock, response_buffer + sizeof(RpcResponseHeader), resp_header->val_len, MSG_WAITALL);
            if (received <= 0) {
                fprintf(stderr, "recv body failed (i=%d). Error: %s\n", i, strerror(errno));
                break;
            }
        }

        clock_gettime(CLOCK_MONOTONIC, &end);
        
        if (resp_header->status != 0) {
             fprintf(stderr, "Server returned error status: %d\n", resp_header->status);
        }

        long long elapsed_ns = (long long)(end.tv_sec - start.tv_sec) * 1000000000LL + (end.tv_nsec - start.tv_nsec);
        total_time_ns += elapsed_ns;
    }

    if (total_time_ns > 0) {
        double avg_latency_us = (double)total_time_ns / num_requests / 1000.0;
        printf("Requests: %d\n", num_requests);
        printf("time: %.3f ms\n", (double)total_time_ns / 1000000.0);
        printf("RPC latency: %.2f us\n", avg_latency_us);
        printf("Throughput: %.2f req/s\n", (double)num_requests / (total_time_ns / 1000000000.0));
    }
    
    free(request_buffer);
}

int main() {
    int sock = 0;
    struct sockaddr_in serv_addr;
    
    if ((sock = socket(AF_INET, SOCK_STREAM, 0)) < 0) {
        perror("Socket creation error");
        return -1;
    }

    serv_addr.sin_family = AF_INET;
    serv_addr.sin_port = htons(SERVER_PORT);
    
    if (inet_pton(AF_INET, SERVER_IP, &serv_addr.sin_addr) <= 0) {
        perror("Invalid address/ Address not supported");
        close(sock);
        return -1;
    }

    if (connect(sock, (struct sockaddr *)&serv_addr, sizeof(serv_addr)) < 0) {
        perror("Connection Failed");
        close(sock);
        return -1;
    }
    
    printf("Connection success (%s:%d)\n", SERVER_IP, SERVER_PORT);
    
    const char* key = "FPGA_RPC_TEST_KEY";

    for (int i = 0; i < 100000; i ++) {
        measure_rpc_performance(sock, OP_STORE, key, random_string(1000).c_str());
        measure_rpc_performance(sock, OP_GET, key, NULL);
    }


    close(sock);
    return 0;
}