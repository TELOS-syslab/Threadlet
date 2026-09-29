#include <stdio.h>
#include <stdlib.h>
#include <string>
#include <unistd.h>
#include <pthread.h>
#include <sys/socket.h>
#include <arpa/inet.h>
#include <time.h>
#include <errno.h>
#include <cstring>
#include <algorithm>

#define OP_STORE 1
#define OP_GET   2

#define SERVER_IP "127.0.0.1"
#define SERVER_PORT 8888
#define NUM_CLIENT_THREADS 1000
#define REQUESTS_PER_THREAD 100

typedef struct {
    int operation;
    int key_len;
    int val_len;
} RpcRequestHeader;

typedef struct {
    int status;
    int val_len;
} RpcResponseHeader;

typedef struct {
    int thread_id;
    int requst_numbers;
    const char* key;
    const char* value;
} ThreadArgs;


void perform_rpc_and_measure(int sock, int op_type, const char* key_str, const char* val_str, long long* total_time_ns) {
    size_t key_len = strlen(key_str);
    size_t val_len = (val_str == NULL) ? 0 : strlen(val_str);
    size_t data_payload_len = key_len + val_len;
    size_t total_request_len = sizeof(RpcRequestHeader) + data_payload_len;

    char request_buffer[2048];
    if (total_request_len > sizeof(request_buffer)) {
        fprintf(stderr, "Request size exceeds buffer limit.\n");
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
    
    struct timespec start, end;
    char response_buffer[2048];

    clock_gettime(CLOCK_MONOTONIC, &start);

    if (send(sock, request_buffer, total_request_len, 0) < 0) {
        perror("send failed");
        return;
    }

    ssize_t received = recv(sock, response_buffer, sizeof(RpcResponseHeader), MSG_WAITALL);
    if (received <= 0) {
        fprintf(stderr, "recv header failed. Error: %s\n", strerror(errno));
        return;
    }

    RpcResponseHeader* resp_header = (RpcResponseHeader*)response_buffer;

    if (resp_header->val_len > 0) {
        received = recv(sock, response_buffer + sizeof(RpcResponseHeader), resp_header->val_len, MSG_WAITALL);
        if (received <= 0) {
            fprintf(stderr, "recv body failed. Error: %s\n", strerror(errno));
            return;
        }
    }
    
    clock_gettime(CLOCK_MONOTONIC, &end);
    
    *total_time_ns += (long long)(end.tv_sec - start.tv_sec) * 1000000000LL + (end.tv_nsec - start.tv_nsec);
    
    if (resp_header->status != 0 && resp_header->status != -1) {
         fprintf(stderr, "Server returned error status: %d\n", resp_header->status);
    }
}

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

void* client_thread_func(void* arg) {

    ThreadArgs* args = (ThreadArgs*)arg;
    int thread_id = args->thread_id;
    int request_numbers = args->requst_numbers;

    int sock = 0;
    struct sockaddr_in serv_addr;
    long long thread_total_time_ns = 0;

    if ((sock = socket(AF_INET, SOCK_STREAM, 0)) < 0) {
        perror("Socket creation error in thread");
        return NULL;
    }

    serv_addr.sin_family = AF_INET;
    serv_addr.sin_port = htons(SERVER_PORT);
    
    if (inet_pton(AF_INET, SERVER_IP, &serv_addr.sin_addr) <= 0) {
        perror("Invalid address/ Address not supported in thread");
        close(sock);
        return NULL;
    }

    if (connect(sock, (struct sockaddr *)&serv_addr, sizeof(serv_addr)) < 0) {
        fprintf(stderr, "Thread %d: Connection Failed. Error: %s\n", thread_id, strerror(errno));
        close(sock);
        return NULL;
    }
    
    printf("Thread %d connected and running %d requests...\n", thread_id, request_numbers);

    for (int i = 0; i < request_numbers; i++) {
        int op_type = (i % 2 == 0) ? OP_STORE : OP_GET;
        const char* val_to_send = (op_type == OP_STORE) ? args->value : NULL;
        perform_rpc_and_measure(sock, op_type, args->key, val_to_send, &thread_total_time_ns);
    }
    
    close(sock);
    
    long long* result_time = (long long*)malloc(sizeof(long long));
    *result_time = thread_total_time_ns;
    return result_time;
}

int main(int argc, char **argv) {
    if (argc != 3) {
        printf("Usage: client [num-of-thread] [num-of-requests]\n");
        return 1;
    }
    int thread_numbers = atoi(argv[1]);
    int request_numbers = atoi(argv[2]);

    pthread_t threads[NUM_CLIENT_THREADS];
    ThreadArgs args[NUM_CLIENT_THREADS];

    for (int i = 0; i < thread_numbers; i ++) {
        char* key = (char*) malloc(20);
        char* value = (char *) malloc(110);
        std::string s = random_string(100);
        sprintf(key, "TEST_KEY %d", i);
        memcpy(value, s.c_str(), s.size());

        args[i].thread_id = i;
        args[i].requst_numbers = request_numbers;
        args[i].key = key;
        args[i].value = value;
    }

    long long total_network_time_ns = 0;
    
    printf("Spwan %d threads, each %d requests\n", thread_numbers, request_numbers);

    for (int i = 0; i < thread_numbers; i++) {
        
        if (pthread_create(&threads[i], NULL, client_thread_func, &args[i]) != 0) {
            perror("Thread creation failed");
            for(int j = 0; j < thread_numbers; j ++) { 
                free((char*)args[j].key);
                free((char*)args[j].value);
            }
            return 1;
        }
    }

    for (int i = 0; i < thread_numbers; i++) {
        long long* result_time;
        pthread_join(threads[i], (void**)&result_time);
        
        if (result_time) {
            total_network_time_ns += *result_time;
            free(result_time);
        }
        free((char*)args[i].key);
        free((char*)args[i].value);
    }
    
    int total_requests = thread_numbers * request_numbers;
    if (total_network_time_ns > 0) {
        double avg_latency_us = (double)total_network_time_ns / total_requests / 1000.0;
        double throughput_req_per_s = (double)total_requests / (total_network_time_ns / 1000000000.0);
        
        printf("requests: %d\n", total_requests);
        printf("RTT: %.3f ms\n", (double)total_network_time_ns / 1000000.0);
        printf("RPC latency Per Request: %.2f us\n", avg_latency_us);
        printf("Throughput: %.2f req/s\n", throughput_req_per_s);
    }

    return 0;
}