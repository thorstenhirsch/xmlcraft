pub(crate) const WINDOWS_MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0" xmlns:asmv3="urn:schemas-microsoft-com:asm.v3">
  <asmv3:application>
    <asmv3:windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2, PerMonitor</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </asmv3:windowsSettings>
  </asmv3:application>
</assembly>
"#;

#[cfg(test)]
mod tests {
    use super::WINDOWS_MANIFEST;

    #[test]
    fn declares_per_monitor_v2_with_per_monitor_fallback() {
        assert!(
            WINDOWS_MANIFEST
                .contains("<dpiAwareness xmlns=\"http://schemas.microsoft.com/SMI/2016/WindowsSettings\">PerMonitorV2, PerMonitor</dpiAwareness>")
        );
        assert!(WINDOWS_MANIFEST.contains("<dpiAware xmlns=\"http://schemas.microsoft.com/SMI/2005/WindowsSettings\">true/pm</dpiAware>"));
    }

    #[test]
    fn allows_paths_longer_than_260_characters() {
        // Deeply nested schema folders are normal in XML projects.
        assert!(WINDOWS_MANIFEST.contains("<longPathAware xmlns=\"http://schemas.microsoft.com/SMI/2016/WindowsSettings\">true</longPathAware>"));
    }

    #[test]
    fn the_resource_build_script_embeds_this_manifest() {
        let build_script = include_str!("../build.rs");
        assert!(build_script.contains("set_manifest(windows_manifest::WINDOWS_MANIFEST)"));
    }

    #[test]
    fn the_manifest_is_well_formed_enough_to_start_with_a_declaration_and_close_its_root() {
        assert!(WINDOWS_MANIFEST.starts_with("<?xml"));
        assert!(WINDOWS_MANIFEST.trim_end().ends_with("</assembly>"));
    }
}
