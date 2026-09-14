/*! The PostgreSQL credentials the db-sync and validator stages share

`cardano-db-sync` and the Midnight node talk to the same database, so the password is asked
for once and kept in a root-only file under the PostgreSQL data root. Everything which needs
it later reads it from there rather than prompting again.
*/

use anyhow::Context;
use std::collections::HashMap;
use std::path::Path;

use crate::settings::Secret;
use crate::util::urlencode;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct DatabaseCredentials {
    pub user: String,
    pub name: String,
    pub password: Secret,
    pub host: String,
    pub port: u16,
}

impl DatabaseCredentials {
    pub fn new(user: impl Into<String>, name: impl Into<String>, password: Secret) -> Self {
        Self {
            user: user.into(),
            name: name.into(),
            password,
            host: "localhost".into(),
            port: 5432,
        }
    }

    /// The `postgresql://` URL `cardano-db-sync` and the node take, with every component
    /// percent encoded so a password with `@` or `/` in it cannot break the URL
    pub fn connection_string(&self) -> String {
        format!(
            "postgresql://{user}:{password}@{host}:{port}/{name}",
            user = urlencode(&self.user),
            password = urlencode(self.password.expose()),
            host = self.host,
            port = self.port,
            name = urlencode(&self.name),
        )
    }

    /// The file the shell steps knew as `fno-db-credentials.env`, mode `600`, root owned
    pub fn render_env_file(&self) -> String {
        format!(
            "DB_USER={user}\nDB_NAME={name}\nDB_PASS={password}\nDB_HOST={host}\nDB_PORT={port}\n",
            user = shell_quote(&self.user),
            name = shell_quote(&self.name),
            password = shell_quote(self.password.expose()),
            host = shell_quote(&self.host),
            port = self.port,
        )
    }

    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn load(path: &Path) -> anyhow::Result<Self> {
        let contents = tokio::fs::read_to_string(path).await.with_context(|| {
            format!(
                "Reading the saved database credentials `{}`",
                path.display()
            )
        })?;
        let values = parse_env_file(&contents);

        let get = |key: &str| -> anyhow::Result<String> {
            values.get(key).cloned().with_context(|| {
                format!(
                    "The saved database credentials `{}` have no `{key}`",
                    path.display()
                )
            })
        };

        Ok(Self {
            user: get("DB_USER")?,
            name: get("DB_NAME")?,
            password: Secret::new(get("DB_PASS")?),
            host: values
                .get("DB_HOST")
                .cloned()
                .unwrap_or_else(|| "localhost".into()),
            port: values
                .get("DB_PORT")
                .and_then(|port| port.parse().ok())
                .unwrap_or(5432),
        })
    }
}

/// Quote a value for a `KEY=value` line a shell (or systemd) would read back unchanged
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Read `KEY=value` lines, understanding the quoting `printf '%q'` and this installer emit
fn parse_env_file(contents: &str) -> HashMap<String, String> {
    let mut values = HashMap::new();

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, raw)) = line.split_once('=') else {
            continue;
        };
        values.insert(key.trim().to_string(), unquote(raw.trim()));
    }

    values
}

fn unquote(raw: &str) -> String {
    // `$'...'`, which `printf '%q'` emits for values with control characters
    if let Some(inner) = raw.strip_prefix("$'").and_then(|v| v.strip_suffix('\'')) {
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some(other) => out.push(other),
                None => break,
            }
        }
        return out;
    }

    if let Some(inner) = raw.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
        return inner.replace(r"'\''", "'");
    }

    if let Some(inner) = raw.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        return inner.replace("\\\"", "\"").replace("\\\\", "\\");
    }

    // Unquoted, but `printf '%q'` backslash escapes the characters a shell would eat
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    out.push(next)
                }
            },
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn round_trips_a_password_with_shell_metacharacters() {
        let credentials = DatabaseCredentials::new(
            "midnight",
            "cexplorer",
            Secret::new("it's $tricky\\ \"quoted\""),
        );
        let rendered = credentials.render_env_file();
        let parsed = parse_env_file(&rendered);

        assert_eq!(parsed.get("DB_PASS").unwrap(), "it's $tricky\\ \"quoted\"");
        assert_eq!(parsed.get("DB_USER").unwrap(), "midnight");
    }

    #[test]
    fn reads_the_quoting_printf_q_emits() {
        let parsed = parse_env_file("DB_USER=midnight\nDB_PASS=p\\@ss\\ word\nDB_PORT=5432\n");
        assert_eq!(parsed.get("DB_PASS").unwrap(), "p@ss word");
        assert_eq!(parsed.get("DB_PORT").unwrap(), "5432");
    }

    #[test]
    fn percent_encodes_the_connection_string() {
        let credentials =
            DatabaseCredentials::new("midnight", "cexplorer", Secret::new("p@ss:w/rd"));
        assert_eq!(
            credentials.connection_string(),
            "postgresql://midnight:p%40ss%3Aw%2Frd@localhost:5432/cexplorer"
        );
    }

    #[test]
    fn never_shows_the_password_in_debug_output() {
        let credentials = DatabaseCredentials::new("midnight", "cexplorer", Secret::new("hunter2"));
        assert!(!format!("{credentials:?}").contains("hunter2"));
    }
}
