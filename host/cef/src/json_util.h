#pragma once

#include <cstdlib>
#include <string>

// Tiny helpers for our fixed IPC JSON shape — no third-party JSON lib.

inline std::string JsonEscape(const std::string& s) {
  std::string out;
  out.reserve(s.size() + 8);
  for (char c : s) {
    switch (c) {
      case '\\':
        out += "\\\\";
        break;
      case '"':
        out += "\\\"";
        break;
      case '\n':
        out += "\\n";
        break;
      case '\r':
        out += "\\r";
        break;
      case '\t':
        out += "\\t";
        break;
      default:
        out += c;
        break;
    }
  }
  return out;
}

inline bool JsonFindString(const std::string& json, const char* key, std::string* out) {
  const std::string needle = std::string("\"") + key + "\"";
  size_t pos = json.find(needle);
  if (pos == std::string::npos) return false;
  pos = json.find(':', pos + needle.size());
  if (pos == std::string::npos) return false;
  pos = json.find('"', pos + 1);
  if (pos == std::string::npos) return false;
  size_t end = pos + 1;
  std::string value;
  while (end < json.size()) {
    char c = json[end];
    if (c == '\\' && end + 1 < json.size()) {
      value.push_back(json[end + 1]);
      end += 2;
      continue;
    }
    if (c == '"') break;
    value.push_back(c);
    ++end;
  }
  *out = value;
  return true;
}

inline bool JsonFindInt(const std::string& json, const char* key, int* out) {
  const std::string needle = std::string("\"") + key + "\"";
  size_t pos = json.find(needle);
  if (pos == std::string::npos) return false;
  pos = json.find(':', pos + needle.size());
  if (pos == std::string::npos) return false;
  ++pos;
  while (pos < json.size() && (json[pos] == ' ' || json[pos] == '\t')) ++pos;
  char* end = nullptr;
  long v = std::strtol(json.c_str() + pos, &end, 10);
  if (end == json.c_str() + pos) return false;
  *out = static_cast<int>(v);
  return true;
}

/** HWND / handle ids — must not go through signed strtol (high bit → ERANGE). */
inline bool JsonFindUInt(const std::string& json, const char* key, unsigned int* out) {
  const std::string needle = std::string("\"") + key + "\"";
  size_t pos = json.find(needle);
  if (pos == std::string::npos) return false;
  pos = json.find(':', pos + needle.size());
  if (pos == std::string::npos) return false;
  ++pos;
  while (pos < json.size() && (json[pos] == ' ' || json[pos] == '\t')) ++pos;
  char* end = nullptr;
  unsigned long v = std::strtoul(json.c_str() + pos, &end, 10);
  if (end == json.c_str() + pos) return false;
  *out = static_cast<unsigned int>(v);
  return true;
}

inline bool JsonFindBool(const std::string& json, const char* key, bool* out) {
  const std::string needle = std::string("\"") + key + "\"";
  size_t pos = json.find(needle);
  if (pos == std::string::npos) return false;
  pos = json.find(':', pos + needle.size());
  if (pos == std::string::npos) return false;
  while (pos < json.size() && (json[pos] == ':' || json[pos] == ' ' || json[pos] == '\t')) {
    ++pos;
  }
  if (json.compare(pos, 4, "true") == 0) {
    *out = true;
    return true;
  }
  if (json.compare(pos, 5, "false") == 0) {
    *out = false;
    return true;
  }
  return false;
}

inline std::string JsonGetOp(const std::string& json) {
  std::string op;
  JsonFindString(json, "op", &op);
  return op;
}
