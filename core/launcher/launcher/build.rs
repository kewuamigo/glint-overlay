fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rustc-link-arg=/SUBSYSTEM:WINDOWS");
        let mut res = winres::WindowsResource::new();
        res.set("FileDescription", "Glint Launcher (Administrator required)");
        res.set("ProductName", "Glint");
        res.set("CompanyName", "Glint");
        res.set("LegalCopyright", "MIT OR Apache-2.0");
        res.set_manifest(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
</assembly>"#,
        );
        if let Err(err) = res.compile() {
            panic!("failed to embed Windows manifest: {err}");
        }
    }
}
