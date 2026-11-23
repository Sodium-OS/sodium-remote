// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The Linux distribution's name for the UI (`Hello.client_os_flavor`,
//! `Welcome.host_os_flavor`): the `ID` of os-release(5), lowercase. Empty on
//! Windows and macOS, and on a Linux with no readable os-release.

/// The `ID` value in os-release text: unquoted or quoted, lowercased, empty when
/// absent. Only plain ids are kept (a-z, 0-9, `.`, `_`, `-`), as the UI compares it
/// and never shows it.
pub fn parse_id(text: &str) -> String {
    for line in text.lines() {
        let Some(v) = line.trim().strip_prefix("ID=") else { continue };
        let v = v.trim().trim_matches(|c| c == '"' || c == '\'').to_ascii_lowercase();
        if v.len() <= 64 && !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
            return v;
        }
        return String::new();
    }
    String::new()
}

/// This computer's flavor: `/etc/os-release`, else `/usr/lib/os-release`, on Linux.
pub fn flavor() -> String {
    #[cfg(target_os = "linux")]
    {
        for p in ["/etc/os-release", "/usr/lib/os-release"] {
            if let Ok(t) = std::fs::read_to_string(p) {
                return parse_id(&t);
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::parse_id;

    #[test]
    fn reads_the_id_line() {
        assert_eq!(parse_id("NAME=\"Arch Linux\"\nID=arch\nID_LIKE=x\n"), "arch");
        assert_eq!(parse_id("PRETTY_NAME=\"SodiumOS\"\nID=\"SodiumOS\"\n"), "sodiumos");
        assert_eq!(parse_id("ID_LIKE=debian\nVERSION_ID=12\n"), "");
        assert_eq!(parse_id("ID=bad id\n"), "");
        assert_eq!(parse_id(""), "");
    }
}
