#include <cstdint>
#include <string>

enum OperationType : int32_t {
    OP_STORE = 1,
    OP_GET = 2
};

struct RpcRequestHeader {
    int32_t operation;
    int32_t key_len;
    int32_t val_len;
};

struct RpcResponseHeader {
    int32_t status;
    int32_t val_len;
};