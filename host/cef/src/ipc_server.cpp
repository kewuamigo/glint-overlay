#include "ipc_server.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <winsock2.h>
#include <ws2tcpip.h>

#include <cstdio>
#include <cstring>
#include <vector>

#pragma comment(lib, "ws2_32.lib")

namespace {

bool ReadExact(SOCKET s, void* buf, size_t len) {
  auto* p = static_cast<char*>(buf);
  size_t got = 0;
  while (got < len) {
    const int n = recv(s, p + got, static_cast<int>(len - got), 0);
    if (n <= 0) return false;
    got += static_cast<size_t>(n);
  }
  return true;
}

bool WriteExact(SOCKET s, const void* buf, size_t len) {
  auto* p = static_cast<const char*>(buf);
  size_t sent = 0;
  while (sent < len) {
    const int n = send(s, p + sent, static_cast<int>(len - sent), 0);
    if (n <= 0) return false;
    sent += static_cast<size_t>(n);
  }
  return true;
}

}  // namespace

IpcClient::IpcClient() = default;

IpcClient::~IpcClient() { Stop(); }

bool IpcClient::Start(uint16_t port, ProtoHandler on_proto) {
  if (running_) return true;
  on_proto_ = std::move(on_proto);
  proto_negotiated_.store(false);

  WSADATA wsa{};
  if (WSAStartup(MAKEWORD(2, 2), &wsa) != 0) return false;

  SOCKET sock = INVALID_SOCKET;
  for (int attempt = 0; attempt < 50; ++attempt) {
    sock = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (sock == INVALID_SOCKET) return false;
    BOOL yes = TRUE;
    setsockopt(sock, IPPROTO_TCP, TCP_NODELAY, reinterpret_cast<char*>(&yes), sizeof(yes));

    sockaddr_in addr{};
    addr.sin_family = AF_INET;
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    addr.sin_port = htons(port);
    if (connect(sock, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) == 0) {
      break;
    }
    closesocket(sock);
    sock = INVALID_SOCKET;
    Sleep(100);
  }
  if (sock == INVALID_SOCKET) return false;

  sock_ = static_cast<uintptr_t>(sock);
  running_ = true;

  SendHello();

  read_thread_ = std::thread([this] { ReadLoop(); });
  return true;
}

void IpcClient::SendHello() {
  gameoverlay::cef::Envelope env;
  env.set_protocol_version(gameoverlay::cef::PROTOCOL_VERSION_1);
  env.mutable_hello();
  SendProto(env);
}

void IpcClient::RejectVersion(uint32_t correlation_id, const char* detail) {
  gameoverlay::cef::Envelope env;
  env.set_protocol_version(gameoverlay::cef::PROTOCOL_VERSION_1);
  env.set_correlation_id(correlation_id);
  auto* err = env.mutable_error();
  err->set_correlation_id(correlation_id);
  err->set_code("UNSUPPORTED_VERSION");
  err->set_message(detail ? detail : "unsupported protocol_version");
  SendProto(env);
  std::fprintf(stderr, "cef ipc: %s\n", detail ? detail : "unsupported protocol_version");
}

void IpcClient::Stop() {
  running_ = false;
  if (sock_) {
    closesocket(static_cast<SOCKET>(sock_));
    sock_ = 0;
  }
  if (read_thread_.joinable()) {
    read_thread_.join();
  }
  WSACleanup();
}

void IpcClient::ReadLoop() {
  SOCKET sock = static_cast<SOCKET>(sock_);
  while (running_ && sock) {
    uint32_t len = 0;
    if (!ReadExact(sock, &len, sizeof(len))) break;
    if (len == 0 || len > 64 * 1024 * 1024) break;
    std::vector<uint8_t> buf(len);
    if (!ReadExact(sock, buf.data(), len)) break;
    if (buf.empty()) continue;

    const auto type = static_cast<IpcMsgType>(buf[0]);
    const void* payload = buf.data() + 1;
    const size_t payload_len = buf.size() - 1;

    if (type != IpcMsgType::Proto) {
      std::fprintf(stderr, "cef ipc: rejecting non-PROTO frame type=%u\n",
                   static_cast<unsigned>(buf[0]));
      continue;
    }

    gameoverlay::cef::Envelope env;
    if (!env.ParseFromArray(payload, static_cast<int>(payload_len))) {
      continue;
    }

    if (env.protocol_version() != gameoverlay::cef::PROTOCOL_VERSION_1) {
      RejectVersion(env.correlation_id(), "incompatible Envelope.protocol_version");
      // Close: incompatible negotiation is fatal.
      closesocket(sock);
      sock_ = 0;
      break;
    }

    if (env.has_hello_ack()) {
      proto_negotiated_.store(true);
    }

    if (on_proto_) {
      on_proto_(env);
    }
  }
}

bool IpcClient::WriteFrame(IpcMsgType type, const void* data, size_t len) {
  std::lock_guard<std::mutex> lock(write_mu_);
  SOCKET sock = static_cast<SOCKET>(sock_);
  if (!sock || sock == INVALID_SOCKET) return false;
  std::vector<uint8_t> frame(4 + 1 + len);
  const uint32_t total = static_cast<uint32_t>(1 + len);
  std::memcpy(frame.data(), &total, 4);
  frame[4] = static_cast<uint8_t>(type);
  if (len > 0) std::memcpy(frame.data() + 5, data, len);
  return WriteExact(sock, frame.data(), frame.size());
}

bool IpcClient::SendProto(const gameoverlay::cef::Envelope& env) {
  std::string bytes;
  if (!env.SerializeToString(&bytes)) return false;
  return WriteFrame(IpcMsgType::Proto, bytes.data(), bytes.size());
}
