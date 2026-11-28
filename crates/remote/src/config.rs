//! Hosts from the user's OpenSSH client config, and `ssh -G` resolution of a [`Target`].

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use crate::host::Target;
use crate::ssh::Ssh;
use crate::{RemoteError, process};

/// OpenSSH's own limit on nested `Include`s.
const MAX_INCLUDE_DEPTH: usize = 16;

/// A concrete `Host` alias from the user's ssh config, connectable as `ssh <alias>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigHost {
    pub alias: String,
}

/// One relevant directive of a config file, in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Entry {
    Alias(String),
    Include(String),
}

/// Lists the concrete host aliases in `<home>/.ssh/config` and the files it `Include`s, in
/// the order ssh reads them, without duplicates.
///
/// Patterns (`*`, `?`), negations (`!host`) and therefore `Host *` are skipped, since they are
/// not something one can connect to. Unreadable or missing files contribute nothing.
pub fn discover_hosts(home: &Path) -> Vec<ConfigHost> {
    let mut hosts = Vec::new();
    let mut seen = HashSet::new();
    walk(
        &home.join(".ssh").join("config"),
        home,
        0,
        &mut hosts,
        &mut seen,
    );
    hosts
}

fn walk(
    file: &Path,
    home: &Path,
    depth: usize,
    hosts: &mut Vec<ConfigHost>,
    seen: &mut HashSet<String>,
) {
    if depth > MAX_INCLUDE_DEPTH {
        log::warn!(
            "ssh config includes nest deeper than {MAX_INCLUDE_DEPTH} at {}",
            file.display()
        );
        return;
    }
    let Ok(text) = fs::read_to_string(file) else {
        return;
    };
    for entry in entries(&text) {
        match entry {
            Entry::Alias(alias) => {
                if seen.insert(alias.clone()) {
                    hosts.push(ConfigHost { alias });
                }
            }
            Entry::Include(pattern) => {
                for path in expand_include(&pattern, home) {
                    walk(&path, home, depth + 1, hosts, seen);
                }
            }
        }
    }
}

/// Parses one config file's text into its concrete host aliases and its `Include` patterns.
///
/// Keywords are case-insensitive and may be separated from their arguments by whitespace or
/// `=`; arguments may be quoted; `#` starts a comment.
pub fn parse_config(text: &str) -> (Vec<String>, Vec<String>) {
    let mut aliases = Vec::new();
    let mut includes = Vec::new();
    for entry in entries(text) {
        match entry {
            Entry::Alias(alias) => aliases.push(alias),
            Entry::Include(pattern) => includes.push(pattern),
        }
    }
    (aliases, includes)
}

fn entries(text: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim_start();
        let keyword_end = line
            .find(|c: char| c.is_whitespace() || c == '=')
            .unwrap_or(line.len());
        let (keyword, rest) = line.split_at(keyword_end);
        if keyword.is_empty() || keyword.starts_with('#') {
            continue;
        }
        let rest = rest.trim_start();
        let rest = rest.strip_prefix('=').unwrap_or(rest);
        let args = split_args(rest);
        if keyword.eq_ignore_ascii_case("host") {
            entries.extend(
                args.into_iter()
                    .filter(|alias| is_concrete(alias))
                    .map(Entry::Alias),
            );
        } else if keyword.eq_ignore_ascii_case("include") {
            entries.extend(args.into_iter().map(Entry::Include));
        }
    }
    entries
}

fn is_concrete(alias: &str) -> bool {
    !alias.is_empty() && !alias.starts_with('!') && !alias.contains(['*', '?'])
}

/// Splits arguments the way OpenSSH does: whitespace-separated, `"…"`/`'…'` quoting,
/// backslash escapes for quotes, and an unquoted `#` at the start of a word ends the line.
fn split_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        match chars.peek() {
            None | Some('#') => break,
            Some(_) => {}
        }
        let mut arg = String::new();
        let mut quote: Option<char> = None;
        while let Some(c) = chars.next() {
            match quote {
                Some(q) if c == q => quote = None,
                Some(q) if c == '\\' && chars.peek().is_some_and(|&n| n == q || n == '\\') => {
                    arg.extend(chars.next());
                }
                Some(_) => arg.push(c),
                None if c.is_whitespace() => break,
                None if c == '"' || c == '\'' => quote = Some(c),
                None if c == '\\'
                    && chars
                        .peek()
                        .is_some_and(|&n| n.is_whitespace() || matches!(n, '"' | '\'' | '\\')) =>
                {
                    arg.extend(chars.next());
                }
                None => arg.push(c),
            }
        }
        args.push(arg);
    }
    args
}

/// The files an `Include` pattern in the user config names: `~` is the home directory and
/// relative patterns are relative to `~/.ssh`. Glob matches are sorted, as with glob(3).
fn expand_include(pattern: &str, home: &Path) -> Vec<PathBuf> {
    let path = if pattern == "~" {
        home.to_path_buf()
    } else if let Some(rest) = pattern.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(pattern).is_absolute() {
        PathBuf::from(pattern)
    } else {
        home.join(".ssh").join(pattern)
    };
    glob(&path)
        .into_iter()
        .filter(|path| path.is_file())
        .collect()
}

/// Expands `*` and `?` in any component of an absolute path.
fn glob(path: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![PathBuf::new()];
    for component in path.components() {
        let part = component.as_os_str();
        let pattern = match component {
            Component::Normal(name) => name.to_str().filter(|name| name.contains(['*', '?'])),
            _ => None,
        };
        let Some(pattern) = pattern else {
            for candidate in &mut candidates {
                candidate.push(part);
            }
            continue;
        };
        let mut next = Vec::new();
        for dir in &candidates {
            let Ok(read) = fs::read_dir(dir) else {
                continue;
            };
            let mut names: Vec<String> = read
                .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                .filter(|name| !name.starts_with('.') || pattern.starts_with('.'))
                .filter(|name| wildcard_match(pattern, name))
                .collect();
            names.sort();
            next.extend(names.into_iter().map(|name| dir.join(name)));
        }
        candidates = next;
    }
    candidates
}

/// Shell-style match of `*` (any run) and `?` (any one character).
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some('*') => {
                backtrack = Some((p, t));
                p += 1;
            }
            Some(&c) if c == '?' || c == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match backtrack {
                Some((star, matched)) => {
                    p = star + 1;
                    t = matched + 1;
                    backtrack = Some((star, matched + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

/// The effective connection parameters ssh would use for a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub hostname: String,
    pub user: Option<String>,
    pub port: u16,
}

/// [`Resolved`] plus whether the connection goes through a jump host or proxy command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedFull {
    pub resolved: Resolved,
    pub proxied: bool,
    /// `HostKeyAlias`: the name the host key is recorded under instead of the host name.
    pub host_key_alias: Option<String>,
    /// The first `UserKnownHostsFile` (tilde not expanded).
    pub known_hosts: Option<String>,
    /// The key file the config names (tilde not expanded). `None` with several: ssh lists its
    /// own defaults when the config names none.
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
}

/// What the ssh config says about a `Host`, to show in a form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigValues {
    pub hostname: String,
    pub user: Option<String>,
    pub port: u16,
    /// Tilde not expanded.
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
}

/// The connection settings of `alias` in the ssh config (`ssh -G`).
pub fn config_values(alias: &str) -> Result<ConfigValues, RemoteError> {
    let full = resolve_full(&Target::Alias {
        alias: alias.to_owned(),
    })?;
    Ok(ConfigValues {
        hostname: full.resolved.hostname,
        user: full.resolved.user,
        port: full.resolved.port,
        identity_file: full.identity_file,
        proxy_jump: full.proxy_jump,
    })
}

/// Asks `ssh -G` (which evaluates the config without connecting) for the hostname, user and
/// port a target really uses.
pub fn resolve(target: &Target) -> Result<Resolved, RemoteError> {
    resolve_full(target).map(|full| full.resolved)
}

pub(crate) fn resolve_full(target: &Target) -> Result<ResolvedFull, RemoteError> {
    let mut command = platform::command(Ssh::program());
    command.arg("-G").args(Ssh::target_args(target));
    let output = process::run(command, None, Duration::from_secs(10))?;
    if !output.status.success() {
        return Err(RemoteError::Failed(output.stderr_lossy().trim().to_owned()));
    }
    parse_ssh_g(&output.stdout_lossy())
}

/// Parses `ssh -G` output (`keyword value` lines, lowercase keywords).
pub(crate) fn parse_ssh_g(text: &str) -> Result<ResolvedFull, RemoteError> {
    let mut hostname = None;
    let mut user = None;
    let mut port = 22;
    let mut proxied = false;
    let mut host_key_alias = None;
    let mut known_hosts = None;
    let mut identity_files = Vec::new();
    let mut proxy_jump = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        let value = value.trim();
        match key {
            "hostname" => hostname = Some(value.to_owned()),
            "user" => user = Some(value.to_owned()).filter(|user| !user.is_empty()),
            "port" => port = value.parse().unwrap_or(22),
            "proxycommand" => proxied |= value != "none",
            "proxyjump" => {
                proxied |= value != "none";
                proxy_jump = Some(value.to_owned()).filter(|jump| jump != "none");
            }
            "identityfile" => identity_files.push(value.to_owned()),
            "hostkeyalias" => {
                host_key_alias = Some(value.to_owned()).filter(|alias| alias != "none");
            }
            "userknownhostsfile" => {
                known_hosts = value.split_whitespace().next().map(str::to_owned);
            }
            _ => {}
        }
    }
    let hostname =
        hostname.ok_or_else(|| RemoteError::Failed("ssh -G printed no hostname".to_owned()))?;
    Ok(ResolvedFull {
        resolved: Resolved {
            hostname,
            user,
            port,
        },
        proxied,
        host_key_alias,
        known_hosts,
        identity_file: (identity_files.len() == 1)
            .then(|| identity_files.remove(0))
            .filter(|file| file != "none"),
        proxy_jump,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_aliases_includes_and_comments() {
        let text = r#"
# A comment line
Include config.d/*  ~/.orbstack/ssh/config
Host spark spark-2 *.internal !bastion   # trailing comment
    HostName 10.0.0.2
  host=equals-form
HOST = spaced-equals
Host "quoted name" 'single'
Host *
    ServerAliveInterval 30
Host web? db
IdentityFile "~/.ssh/my key"
   include "other config"
Match host foo
"#;
        let (aliases, includes) = parse_config(text);
        assert_eq!(
            aliases,
            strings(&[
                "spark",
                "spark-2",
                "equals-form",
                "spaced-equals",
                "quoted name",
                "single",
                "db",
            ])
        );
        assert_eq!(
            includes,
            strings(&["config.d/*", "~/.orbstack/ssh/config", "other config"])
        );
    }

    #[test]
    fn empty_and_blank_configs() {
        assert_eq!(parse_config(""), (vec![], vec![]));
        assert_eq!(parse_config("\n   \n\t# nothing\n"), (vec![], vec![]));
        assert_eq!(parse_config("Host\n"), (vec![], vec![]));
    }

    #[test]
    fn wildcards() {
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("*.conf", "a.conf"));
        assert!(!wildcard_match("*.conf", "a.conf.bak"));
        assert!(wildcard_match("web?", "web1"));
        assert!(!wildcard_match("web?", "web"));
        assert!(wildcard_match("a*b*c", "aXXbYYc"));
        assert!(!wildcard_match("a*b*c", "aXXbYY"));
    }

    #[test]
    fn discovers_hosts_through_includes() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        fs::create_dir_all(ssh.join("config.d")).unwrap();
        fs::create_dir_all(home.path().join(".orbstack/ssh")).unwrap();
        fs::write(
            ssh.join("config"),
            "Host first\nInclude config.d/*.conf ~/.orbstack/ssh/config missing/*\n\
             Host last first\nInclude config\n",
        )
        .unwrap();
        fs::write(ssh.join("config.d/b.conf"), "Host b-host\n").unwrap();
        fs::write(ssh.join("config.d/a.conf"), "Host a-host *\n").unwrap();
        fs::write(ssh.join("config.d/c.txt"), "Host not-included\n").unwrap();
        fs::write(ssh.join("config.d/.hidden.conf"), "Host hidden\n").unwrap();
        fs::write(home.path().join(".orbstack/ssh/config"), "Host orb\n").unwrap();

        let aliases: Vec<String> = discover_hosts(home.path())
            .into_iter()
            .map(|host| host.alias)
            .collect();
        // The self-include is cut off by the depth limit and adds no duplicates.
        assert_eq!(
            aliases,
            strings(&["first", "a-host", "b-host", "orb", "last"])
        );
    }

    #[test]
    fn missing_config_yields_nothing() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(discover_hosts(home.path()), vec![]);
    }

    #[test]
    fn parses_ssh_g_output() {
        let text = "host spark\nuser alice\nhostname dgx.example.com\nport 2222\n\
                    proxycommand none\naddressfamily any\nhostkeyalias none\n\
                    userknownhostsfile ~/.ssh/known_hosts ~/.ssh/known_hosts2\n";
        let full = parse_ssh_g(text).unwrap();
        assert_eq!(
            full,
            ResolvedFull {
                resolved: Resolved {
                    hostname: "dgx.example.com".into(),
                    user: Some("alice".into()),
                    port: 2222,
                },
                proxied: false,
                host_key_alias: None,
                known_hosts: Some("~/.ssh/known_hosts".into()),
                identity_file: None,
                proxy_jump: None,
            }
        );
        let jumped = parse_ssh_g("hostname h\nport 22\nproxyjump bastion\n").unwrap();
        assert!(jumped.proxied);
        assert_eq!(jumped.proxy_jump.as_deref(), Some("bastion"));
        // One key file is the config's; several are ssh's defaults.
        let one = parse_ssh_g("hostname h\nidentityfile ~/.ssh/key\n").unwrap();
        assert_eq!(one.identity_file.as_deref(), Some("~/.ssh/key"));
        let defaults =
            parse_ssh_g("hostname h\nidentityfile ~/.ssh/a\nidentityfile ~/.ssh/b\n").unwrap();
        assert_eq!(defaults.identity_file, None);
        assert_eq!(jumped.resolved.user, None);
        assert!(parse_ssh_g("port 22\n").is_err());
    }
}
