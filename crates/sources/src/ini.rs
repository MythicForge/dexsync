//! Minimal `key = value` reader for emulator config files
//! (retroarch.cfg, qt-config.ini, mGBA config.ini). Sections are ignored.

/// First value for `key`, unquoted and trimmed.
pub fn value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"').to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_quoted_and_bare_values() {
        let text = "[ui]\nsavefile_directory = \":/saves\"\nsdmc_directory=/x/sdmc/\nsdmc_directory\\default=false\n";
        assert_eq!(value(text, "savefile_directory").as_deref(), Some(":/saves"));
        assert_eq!(value(text, "sdmc_directory").as_deref(), Some("/x/sdmc/"));
        assert_eq!(value(text, "missing"), None);
    }
}
