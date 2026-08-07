#pragma once

#include "ipc/gen/cef_ipc.pb.h"

#include <atomic>
#include <cstdint>
#include <functional>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

// Length-prefixed framed IPC over TCP (CEF connects outbound to host).
// Frame: [u32 le length][u8 type][payload]
// type 2 = MSG_PROTO (protobuf Envelope)

enum class IpcMsgType : uint8_t {
  Proto = 2,
};

class IpcClient {
 public:
  using ProtoHandler =
      std::function<void(const gameoverlay::cef::Envelope& env)>;

  IpcClient();
  ~IpcClient();

  IpcClient(const IpcClient&) = delete;
  IpcClient& operator=(const IpcClient&) = delete;

  bool Start(uint16_t port, ProtoHandler on_proto);
  void Stop();

  bool SendProto(const gameoverlay::cef::Envelope& env);

  bool proto_negotiated() const { return proto_negotiated_.load(); }

 private:
  void ReadLoop();
  bool WriteFrame(IpcMsgType type, const void* data, size_t len);
  void SendHello();
  void RejectVersion(uint32_t correlation_id, const char* detail);

  ProtoHandler on_proto_;
  uintptr_t sock_ = 0;
  std::thread read_thread_;
  std::mutex write_mu_;
  bool running_ = false;
  std::atomic<bool> proto_negotiated_{false};
};
