#include <iostream>
#include <string>
#include <unordered_map>
#include <mutex>
#include <thread>
#include <vector>

#include <sys/socket.h>
#include <netinet/in.h>
#include <unistd.h>
#include <cstring>

#include "RpcProtocol.h"

#define SERVER_PORT 8888
#define BUFFER_SIZE 2048

#define print_enable() { \
	asm volatile ("nop"); \
	asm volatile (".insn r 0x0B, 0, 0x0, x0, x0, x0" ::); \
	asm volatile ("nop"); \
}

#define print_disable() { \
	asm volatile ("nop"); \
	asm volatile (".insn r 0x0B, 0, 0x2, x0, x0, x0" ::); \
	asm volatile ("nop"); \
}

#define S_S_HARD_INTERRUPT_START     0
#define S_S_HARD_INTERRUPT_FINISHED  1
#define S_S_SCHEDULING_START         2
#define S_S_SCHEDULING_FINISHED      3
#define S_S_SOFT_INTERRUPT_START     4
#define S_S_SOFT_INTERRUPT_FINISHED  5
#define S_S_UNKNOWN_1                6
#define S_S_UNKNOWN_2                7
#define S_S_UNKNOWN_3                8
#define S_S_UNKNOWN_4                9

static void hardware_log(int stage, uint64_t data) {
	// print_enable();
	asm volatile ("nop");
	asm volatile (".insn r 0x0B, 0, 0xe, x0, %0, %1" ::"r"(stage), "r"(data));
	asm volatile ("nop");
	// print_disable();
}

std::unordered_map<std::string, std::string> kv_store;
std::mutex kv_mutex;

void handle_client(int client_sock) {
    char buffer[BUFFER_SIZE];
    ssize_t bytes_read;
    
    while ((bytes_read = recv(client_sock, buffer, BUFFER_SIZE, 0)) > 0) {
        // hardware_log(S_S_UNKNOWN_2, 0);
        
        if (bytes_read < sizeof(RpcRequestHeader)) {
            break;
        }
        RpcRequestHeader* req_header = (RpcRequestHeader*)buffer;
        
        size_t expected_data_len = req_header->key_len + req_header->val_len;
        if (bytes_read < sizeof(RpcRequestHeader) + expected_data_len) {
             std::cerr << "Warning: Incomplete RPC data received." << std::endl;
             break;
        }

        const char* data_ptr = buffer + sizeof(RpcRequestHeader);
        std::string key(data_ptr, req_header->key_len);
        std::string value;
        if (req_header->val_len > 0) {
            value = std::string(data_ptr + req_header->key_len, req_header->val_len);
        }

        RpcResponseHeader resp_header;
        resp_header.status = 0;
        resp_header.val_len = 0;
        
        if (req_header->operation == OP_STORE) {
            std::lock_guard<std::mutex> lock(kv_mutex);
            kv_store[key] = value;
        
        } else if (req_header->operation == OP_GET) {
            std::lock_guard<std::mutex> lock(kv_mutex);
            auto it = kv_store.find(key);

            if (it != kv_store.end()) {
                const std::string& found_value = it->second;
                resp_header.val_len = found_value.length();
                
                char response_buffer[BUFFER_SIZE];
                memcpy(response_buffer, &resp_header, sizeof(resp_header));
                memcpy(response_buffer + sizeof(resp_header), found_value.data(), found_value.length());

                send(client_sock, response_buffer, sizeof(resp_header) + found_value.length(), 0);
                continue;
            } else {
                resp_header.status = -1;
            }
        } else {
            resp_header.status = -1;
        }
        
        send(client_sock, &resp_header, sizeof(resp_header), 0);
    }
    
    std::cout << "Client disconnected on thread: " << std::this_thread::get_id() << std::endl;
    close(client_sock);
    // hardware_log(S_S_UNKNOWN_3, 0);
}

int main() {
    int listen_fd = 0, client_fd = 0;
    struct sockaddr_in serv_addr;

    if ((listen_fd = socket(AF_INET, SOCK_STREAM, 0)) < 0) {
        perror("Socket creation failed");
        return 1;
    }
    
    int opt = 1;
    setsockopt(listen_fd, SOL_SOCKET, SO_REUSEADDR, &opt, sizeof(opt));

    serv_addr.sin_family = AF_INET;
    serv_addr.sin_addr.s_addr = htonl(INADDR_ANY);
    serv_addr.sin_port = htons(SERVER_PORT);

    if (bind(listen_fd, (struct sockaddr*)&serv_addr, sizeof(serv_addr)) < 0) {
        perror("Bind failed");
        return 1;
    }

    if (listen(listen_fd, 1000) < 0) {
        perror("Listen failed");
        return 1;
    }

    std::cout << "FPGA Server listening on port " << SERVER_PORT << "..." << std::endl;

    while (1) {
        struct sockaddr_in client_addr;
        socklen_t client_len = sizeof(client_addr);
        
        client_fd = accept(listen_fd, (struct sockaddr*)&client_addr, &client_len);
        // hardware_log(S_S_UNKNOWN_1, 0);
        if (client_fd < 0) {
            perror("Accept failed");
            continue;
        }

        std::thread client_thread(handle_client, client_fd);
        client_thread.detach();
        
        std::cout << "New client connected. Thread ID: " << client_thread.get_id() << std::endl;
    }

    close(listen_fd);
    return 0;
}