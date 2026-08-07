# Compatibility

## In-game overlay host

The supported in-game overlay is **CEF-backed** (`glint-browser` +
`glint-cef.exe`), composited by the injected overlay DLL. There is no
in-game Electron overlay host. The launcher stays Electron for attach and
orchestration only.

See [specs/002-cef-overlay-host/spec.md](../specs/002-cef-overlay-host/spec.md)
and [session-ownership](../specs/002-cef-overlay-host/contracts/session-ownership.md).

## Anti-cheat and protected titles

Best-effort only. Kernel anti-cheat or protected titles may block injection or
overlay compositing. Run the launcher as Administrator when attach fails on
otherwise supported samples. Honest “unsupported” notes beat silent failure.
