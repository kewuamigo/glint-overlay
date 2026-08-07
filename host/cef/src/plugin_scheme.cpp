#include "plugin_scheme.h"

#include "include/cef_parser.h"
#include "include/wrapper/cef_stream_resource_handler.h"

#include <algorithm>

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

namespace plugin_scheme {

const char kScheme[] = "glint-plugin";

namespace {

/** Written once before CefInitialize, read-only on the IO thread after that. */
std::vector<std::wstring> g_app_roots;
std::wstring g_shared_dir;

bool IsSeparator(wchar_t c) {
  return c == L'\\' || c == L'/';
}

/** Absolute path with `..` collapsed; empty when Win32 cannot resolve it. */
std::wstring FullPath(const std::wstring& path) {
  wchar_t buf[4096] = {};
  const DWORD n = GetFullPathNameW(path.c_str(), ARRAYSIZE(buf), buf, nullptr);
  if (n == 0 || n >= ARRAYSIZE(buf)) return std::wstring();
  std::wstring out(buf, n);
  while (!out.empty() && IsSeparator(out.back())) out.pop_back();
  return out;
}

bool IsDirectory(const std::wstring& path) {
  const DWORD attrs = GetFileAttributesW(path.c_str());
  return attrs != INVALID_FILE_ATTRIBUTES && (attrs & FILE_ATTRIBUTE_DIRECTORY) != 0;
}

/** Containment guard from plugin-protocol.ts: a `..` in the URL must not
 *  escape the app dir. */
bool Contains(const std::wstring& dir, const std::wstring& file) {
  if (dir.empty() || file.size() <= dir.size()) return false;
  if (_wcsnicmp(dir.c_str(), file.c_str(), dir.size()) != 0) return false;
  return IsSeparator(file[dir.size()]);
}

/** Same id charset the registry validates, so the host cannot be `..`. */
bool ValidAppId(const std::wstring& host) {
  if (host.empty()) return false;
  return std::all_of(host.begin(), host.end(), [](wchar_t c) {
    return (c >= L'a' && c <= L'z') || (c >= L'0' && c <= L'9') || c == L'-';
  });
}

/** Dynamic `import()` of the bundle fails without the JS type. */
const char* MimeForPath(const std::wstring& path) {
  const size_t dot = path.find_last_of(L'.');
  if (dot == std::wstring::npos) return "application/octet-stream";
  std::wstring ext = path.substr(dot);
  std::transform(ext.begin(), ext.end(), ext.begin(), ::towlower);
  if (ext == L".js" || ext == L".mjs") return "application/javascript";
  if (ext == L".css") return "text/css";
  if (ext == L".json") return "application/json";
  if (ext == L".html") return "text/html";
  if (ext == L".svg") return "image/svg+xml";
  if (ext == L".png") return "image/png";
  if (ext == L".jpg" || ext == L".jpeg") return "image/jpeg";
  if (ext == L".gif") return "image/gif";
  if (ext == L".webp") return "image/webp";
  return "application/octet-stream";
}

/** file:// shell page → opaque origin, so every response needs CORS; without
 *  it a miss surfaces to the app as an opaque fetch failure, not its status. */
CefResponse::HeaderMap CorsHeaders() {
  CefResponse::HeaderMap headers;
  headers.insert(std::make_pair("Access-Control-Allow-Origin", "*"));
  return headers;
}

CefRefPtr<CefResourceHandler> Fail(int status, const char* status_text) {
  std::string body(status_text);
  return new CefStreamResourceHandler(
      status, status_text, "text/plain", CorsHeaders(),
      CefStreamReader::CreateForData(body.data(), body.size()));
}

/** App dirs for this id, in root precedence order. All of them, not just the
 *  first: an internal-apps dir the registry rejects (no `builtin`, missing
 *  entry) must not shadow the %APPDATA% install apps.list actually offers. */
std::vector<std::wstring> BaseDirsFor(const std::wstring& host) {
  std::vector<std::wstring> dirs;
  // Empty shared dir must stay unresolvable — FullPath("") would be the cwd.
  if (host == L"_shared") {
    if (!g_shared_dir.empty()) dirs.push_back(FullPath(g_shared_dir));
    return dirs;
  }
  if (!ValidAppId(host)) return dirs;
  for (const std::wstring& root : g_app_roots) {
    std::wstring dir = FullPath(root + L"\\" + host);
    if (IsDirectory(dir)) dirs.push_back(dir);
  }
  return dirs;
}

class PluginSchemeFactory : public CefSchemeHandlerFactory {
 public:
  CefRefPtr<CefResourceHandler> Create(CefRefPtr<CefBrowser> /*browser*/,
                                       CefRefPtr<CefFrame> /*frame*/,
                                       const CefString& /*scheme_name*/,
                                       CefRefPtr<CefRequest> request) override {
    CefURLParts parts;
    if (!request || !CefParseURL(request->GetURL(), parts)) {
      return Fail(400, "bad request");
    }
    const std::vector<std::wstring> bases =
        BaseDirsFor(CefString(&parts.host).ToWString());
    if (bases.empty()) return Fail(404, "not found");

    std::wstring rel =
        CefURIDecode(CefString(&parts.path), true,
                     static_cast<cef_uri_unescape_rule_t>(UU_NORMAL | UU_SPACES |
                                                          UU_PATH_SEPARATORS))
            .ToWString();
    std::replace(rel.begin(), rel.end(), L'/', L'\\');
    size_t start = 0;
    while (start < rel.size() && IsSeparator(rel[start])) ++start;
    rel = rel.substr(start);

    for (const std::wstring& base : bases) {
      const std::wstring file = FullPath(base + L"\\" + rel);
      if (!Contains(base, file)) return Fail(403, "forbidden");
      CefRefPtr<CefStreamReader> stream = CefStreamReader::CreateForFile(file);
      if (stream) {
        return new CefStreamResourceHandler(200, "OK", MimeForPath(file),
                                            CorsHeaders(), stream);
      }
    }
    return Fail(404, "not found");
  }

  IMPLEMENT_REFCOUNTING(PluginSchemeFactory);
};

}  // namespace

void SetRoots(std::vector<std::wstring> app_roots, std::wstring shared_dir) {
  g_app_roots = std::move(app_roots);
  g_shared_dir = std::move(shared_dir);
}

void Register(CefRawPtr<CefSchemeRegistrar> registrar) {
  // Same privileges as the Electron registerSchemesAsPrivileged entry.
  registrar->AddCustomScheme(kScheme, CEF_SCHEME_OPTION_STANDARD |
                                          CEF_SCHEME_OPTION_SECURE |
                                          CEF_SCHEME_OPTION_CORS_ENABLED |
                                          CEF_SCHEME_OPTION_FETCH_ENABLED);
}

void InstallFactory() {
  CefRegisterSchemeHandlerFactory(kScheme, CefString(), new PluginSchemeFactory());
}

}  // namespace plugin_scheme
