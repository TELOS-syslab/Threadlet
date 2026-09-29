#include <iostream>
#include <vector>
#include <queue>
#include <thread>
#include <mutex>
#include <condition_variable>
#include <functional>
#include <unordered_map>
#include <sys/socket.h>
#include <netinet/in.h>
#include <unistd.h>
#include <cstring>
#include "RpcProtocol.h"

#define SERVER_PORT 8888
#define THREAD_POOL_SIZE 1000

std::unordered_map<std::string, std::string> kv_store;
std::mutex kv_mutex;

class ThreadPool {
public:
    ThreadPool(size_t threads) : stop(false) {
        for(size_t i = 0; i < threads; ++i)
            workers.emplace_back([this] {
                for(;;) {
                    int client_sock;
                    {
                        std::unique_lock<std::mutex> lock(this->queue_mutex);
                        this->condition.wait(lock, [this]{ return this->stop || !this->tasks.empty(); });
                        if(this->stop && this->tasks.empty()) return;
                        
                        client_sock = this->tasks.front();
                        this->tasks.pop();
                    }
                    this->handle_rpc(client_sock);
                }
            });
    }

    void enqueue(int sock) {
        {
            std::unique_lock<std::mutex> lock(queue_mutex);
            tasks.push(sock);
        }
        condition.notify_one();
    }

    ~ThreadPool() {
        {
            std::unique_lock<std::mutex> lock(queue_mutex);
            stop = true;
        }
        condition.notify_all();
        for(std::thread &worker: workers) worker.join();
    }

private:
    std::vector<std::thread> workers;
    std::queue<int> tasks;
    std::mutex queue_mutex;
    std::condition_variable condition;
    bool stop;

    void handle_rpc(int client_sock) {
        char buffer[2048];
        while (true) {
            ssize_t bytes_read = recv(client_sock, buffer, sizeof(RpcRequestHeader), MSG_WAITALL);
            if (bytes_read <= 0) break;

            RpcRequestHeader* req = (RpcRequestHeader*)buffer;
            
            int data_len = req->key_len + req->val_len;
            recv(client_sock, buffer + sizeof(RpcRequestHeader), data_len, MSG_WAITALL);

            process_request(client_sock, req, buffer + sizeof(RpcRequestHeader));
        }
        close(client_sock);
    }

    void process_request(int sock, RpcRequestHeader* req, char* data) {
        RpcResponseHeader resp;
        resp.status = 0;
        resp.val_len = 0;

        std::string key(data, req->key_len);
        
        if (req->operation == OP_STORE) {
            std::string val(data + req->key_len, req->val_len);
            std::lock_guard<std::mutex> lock(kv_mutex);
            kv_store[key] = val;
        } else if (req->operation == OP_GET) {
            std::lock_guard<std::mutex> lock(kv_mutex);
            if (kv_store.count(key)) {
                std::string& found = kv_store[key];
                resp.val_len = found.length();
                send(sock, &resp, sizeof(resp), 0);
                send(sock, found.data(), found.length(), 0);
                return;
            } else {
                resp.status = -1;
            }
        }
        send(sock, &resp, sizeof(resp), 0);
    }
};

int main() {
    int listen_fd = socket(AF_INET, SOCK_STREAM, 0);
    int opt = 1;
    setsockopt(listen_fd, SOL_SOCKET, SO_REUSEADDR, &opt, sizeof(opt));

    struct sockaddr_in addr;
    addr.sin_family = AF_INET;
    addr.sin_port = htons(SERVER_PORT);
    addr.sin_addr.s_addr = INADDR_ANY;

    bind(listen_fd, (struct sockaddr*)&addr, sizeof(addr));
    listen(listen_fd, 64);

    ThreadPool pool(THREAD_POOL_SIZE);

    std::cout << "RPC Server running with " << THREAD_POOL_SIZE << " threads..." << std::endl;

    while (true) {
        int client_fd = accept(listen_fd, NULL, NULL);
        if (client_fd >= 0) {
            pool.enqueue(client_fd);
        }
    }

    return 0;
}