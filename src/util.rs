/*! Small helpers: the framework's filesystem and download utilities, plus the quoting rules
this installer's own file formats need

Everything general — writing a file atomically, resolving an owner, fetching and unpacking a
release — comes from [`installer::util`]. What stays here is the escaping which belongs to
the formats an FNO host is configured with: a URL, a systemd `EnvironmentFile`, a `.pgpass`
line, a SQL literal.
*/

pub(crate) use installer::util::{
    chown_recursive, copy_dir_all, directory_has_contents, fetch_string, file_has_contents, gid_of,
    os_release_codename, remove_dir_all_if_exists, remove_file_if_exists, uid_of, write_owned_file,
};

/// Percent encode a value for use inside a URL (`urllib.parse.quote(safe='')`)
pub(crate) fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            },
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Quote a value for the right hand side of a systemd `EnvironmentFile` assignment
pub(crate) fn systemd_env_quote(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Escape a field of a `.pgpass` line
pub(crate) fn pgpass_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace(':', "\\:")
}

/// Escape a string literal for a SQL statement
pub(crate) fn sql_escape(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn urlencodes_reserved_characters() {
        assert_eq!(urlencode("p@ss:w/rd"), "p%40ss%3Aw%2Frd");
        assert_eq!(urlencode("plain-value_1.0~"), "plain-value_1.0~");
    }

    #[test]
    fn escapes_pgpass_fields() {
        assert_eq!(pgpass_escape("a:b\\c"), "a\\:b\\\\c");
    }

    #[test]
    fn quotes_systemd_environment_values() {
        assert_eq!(systemd_env_quote("a\"b"), "\"a\\\"b\"");
    }

    #[test]
    fn escapes_sql_literals() {
        assert_eq!(sql_escape("O'Brien"), "O''Brien");
    }
}
