//! What the app knows about common Windows processes: a friendly name, what kind of
//! program it is, and whether it is safe to close.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Browser,
    /// Core Windows component. Never offer to close it.
    System,
    Antivirus,
    WindowsUpdate,
    SearchIndexer,
    /// Windows compresses RAM when it runs low. A symptom, not a cause.
    MemoryCompression,
    Other,
}

#[derive(Clone, Copy, Debug)]
pub struct Known {
    pub friendly: &'static str,
    pub kind: Kind,
}

/// Lower-case name without `.exe`, so `Chrome.EXE` and `chrome` match.
pub fn normalize(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    lower
        .strip_suffix(".exe")
        .map(str::to_owned)
        .unwrap_or(lower)
}

pub fn lookup(name: &str) -> Known {
    use Kind::*;
    let (friendly, kind) = match normalize(name).as_str() {
        "chrome" => ("Google Chrome", Browser),
        "msedge" => ("Microsoft Edge", Browser),
        "firefox" => ("Firefox", Browser),
        "opera" | "opera_gx" => ("Opera", Browser),
        "brave" => ("Brave", Browser),
        "vivaldi" => ("Vivaldi", Browser),
        "msmpeng" | "mpdefendercoreservice" => ("Microsoft Defender Antivirus", Antivirus),
        "nissrv" => ("Microsoft Defender Network Inspection", Antivirus),
        "tiworker" | "trustedinstaller" | "wuauclt" | "usoclient" | "mousocoreworker" => {
            ("Windows Update", WindowsUpdate)
        }
        "searchindexer" | "searchprotocolhost" | "searchfilterhost" => {
            ("Windows Search indexer", SearchIndexer)
        }
        "memory compression" => ("Memory Compression", MemoryCompression),
        "system"
        | "idle"
        | "system idle process"
        | "registry"
        | "secure system"
        | "smss"
        | "csrss"
        | "wininit"
        | "winlogon"
        | "services"
        | "lsass"
        | "lsaiso"
        | "svchost"
        | "fontdrvhost"
        | "spoolsv"
        | "audiodg"
        | "sihost"
        | "ctfmon"
        | "taskhostw"
        | "conhost"
        | "wudfhost"
        | "dllhost"
        | "systemd"
        | "init"
        | "kthreadd" => ("Windows system process", System),
        "dwm" => ("Desktop Window Manager (draws your screen)", System),
        "explorer" => ("Windows Explorer (taskbar and desktop)", System),
        "code" => ("Visual Studio Code", Other),
        "teams" | "ms-teams" => ("Microsoft Teams", Other),
        "discord" => ("Discord", Other),
        "steam" => ("Steam", Other),
        "steamwebhelper" => ("Steam web helper", Other),
        "spotify" => ("Spotify", Other),
        "onedrive" => ("OneDrive", Other),
        "python" | "pythonw" => ("Python", Other),
        "java" | "javaw" => ("Java", Other),
        "node" => ("Node.js", Other),
        _ => ("", Other),
    };
    Known { friendly, kind }
}

/// Display name: the friendly name when known, otherwise the executable name.
pub fn display_name(name: &str) -> String {
    let k = lookup(name);
    if k.friendly.is_empty() || k.kind == Kind::System {
        name.to_string()
    } else {
        k.friendly.to_string()
    }
}

/// Processes the app must never offer to close.
pub fn is_protected(name: &str, pids: &[u32], own_pid: u32) -> bool {
    let k = lookup(name).kind;
    matches!(
        k,
        Kind::System | Kind::Antivirus | Kind::WindowsUpdate | Kind::MemoryCompression
    ) || pids.contains(&own_pid)
        || pids.iter().any(|&p| p <= 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_names() {
        assert_eq!(normalize("Chrome.EXE"), "chrome");
        assert_eq!(normalize("Memory Compression"), "memory compression");
    }

    #[test]
    fn protects_system_processes() {
        assert!(is_protected("svchost.exe", &[900], 1));
        assert!(is_protected("csrss.exe", &[600], 1));
        assert!(is_protected("MsMpEng.exe", &[3000], 1));
        assert!(is_protected("System", &[4], 1));
        assert!(!is_protected("chrome.exe", &[5000, 5001], 1));
        // never close ourselves
        assert!(is_protected("pc-bottleneck-finder.exe", &[42], 42));
    }

    #[test]
    fn friendly_names() {
        assert_eq!(display_name("chrome.exe"), "Google Chrome");
        assert_eq!(display_name("svchost.exe"), "svchost.exe");
        assert_eq!(display_name("myapp.exe"), "myapp.exe");
    }
}
