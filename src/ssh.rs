use anyhow::anyhow;
use glob::glob;
use handlebars::Handlebars;
use itertools::Itertools;
use serde::Serialize;
use ssh_config::host::LocalForward;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Command;

use crate::ssh_config::{self, parser_error::ParseError, HostVecExt};

#[derive(Debug, Serialize, Clone)]
pub struct Host {
    pub name: String,
    pub aliases: String,
    pub user: Option<String>,
    pub destination: String,
    pub port: Option<String>,
    pub proxy_command: Option<String>,
    pub proxy_jump: Option<String>,
    pub identity_file: Option<String>,
    pub local_forwards: Vec<LocalForward>,
}

impl Host {
    /// Uses the provided Handlebars template to run a command.
    ///
    /// # Errors
    ///
    /// Will return `Err` if the command cannot be executed.
    ///
    /// # Panics
    ///
    /// Will panic if the regex cannot be compiled.
    pub fn run_command_template(&self, pattern: &str) -> anyhow::Result<()> {
        let handlebars = Handlebars::new();
        let rendered_command = handlebars.render_template(pattern, &self)?;

        println!("Running command: {rendered_command}");

        let mut args = shlex::split(&rendered_command)
            .ok_or(anyhow!("Failed to parse command: {rendered_command}"))?
            .into_iter()
            .collect::<VecDeque<String>>();
        let command = args.pop_front().ok_or(anyhow!("Failed to get command"))?;

        let status = Command::new(command).args(args).spawn()?.wait()?;
        if !status.success() {
            std::process::exit(status.code().unwrap_or(1));
        }

        Ok(())
    }
}

#[derive(Debug)]
pub enum ParseConfigError {
    Io(std::io::Error),
    SshConfig(ParseError),
}

impl From<std::io::Error> for ParseConfigError {
    fn from(e: std::io::Error) -> Self {
        ParseConfigError::Io(e)
    }
}

impl From<ParseError> for ParseConfigError {
    fn from(e: ParseError) -> Self {
        ParseConfigError::SshConfig(e)
    }
}

/// Expand a list of raw config path strings into concrete file paths.
///
/// Each entry is tilde-expanded and then evaluated as a glob pattern.
/// Patterns that contain glob metacharacters expand to the files they match
/// (no error if zero files matched). Plain paths are returned as-is so the
/// caller can decide whether a missing file is fatal.
#[must_use]
pub fn expand_config_paths(raw_paths: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();

    for raw in raw_paths {
        let expanded = shellexpand::tilde(raw).to_string();
        let has_glob = expanded.contains(['*', '?', '[']);

        if has_glob {
            match glob(&expanded) {
                Ok(paths) => {
                    for path in paths.flatten() {
                        if path.is_file() {
                            out.push(path);
                        }
                    }
                }
                Err(_) => out.push(PathBuf::from(expanded)),
            }
        } else {
            out.push(PathBuf::from(expanded));
        }
    }

    out
}

/// # Errors
///
/// Will return `Err` if the SSH configuration file cannot be parsed.
pub fn parse_config<P: AsRef<std::path::Path>>(path: P) -> Result<Vec<Host>, ParseConfigError> {
    let path = std::fs::canonicalize(path.as_ref())?;

    let hosts = ssh_config::Parser::new()
        .parse_file(path)?
        .apply_patterns()
        .apply_name_to_empty_hostname()
        .merge_same_hosts()
        .iter()
        .map(|h| Host {
            name: h.get_patterns().first().unwrap_or(&String::new()).clone(),
            aliases: h.get_patterns().iter().skip(1).join(", "),
            user: h.get(&ssh_config::EntryType::User),
            destination: h.get(&ssh_config::EntryType::Hostname).unwrap_or_default(),
            port: h.get(&ssh_config::EntryType::Port),
            proxy_command: h.get(&ssh_config::EntryType::ProxyCommand),
            proxy_jump: h.get(&ssh_config::EntryType::ProxyJump),
            identity_file: h.get(&ssh_config::EntryType::IdentityFile),
            local_forwards: h.local_forwards.clone(),
        })
        .collect();

    Ok(hosts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn write_config<P: AsRef<std::path::Path>>(path: P, contents: &str) {
        let mut file = fs::File::create(path).expect("create test config");
        file.write_all(contents.as_bytes()).expect("write test config");
    }

    #[test]
    fn expand_config_paths_returns_plain_paths_unchanged() {
        let result = expand_config_paths(&[
            "/etc/ssh/ssh_config".to_string(),
            "~/.ssh/config".to_string(),
        ]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], PathBuf::from("/etc/ssh/ssh_config"));
        // tilde was expanded — should not be `~/.ssh/config` literally.
        assert!(!result[1].to_string_lossy().starts_with('~'));
        assert!(result[1].to_string_lossy().ends_with("/.ssh/config"));
    }

    #[test]
    fn expand_config_paths_expands_glob_into_files() {
        let dir = tempdir();
        let cfg_dir = dir.join("config.d");
        fs::create_dir_all(&cfg_dir).unwrap();
        write_config(cfg_dir.join("a.conf"), "Host a\n  Hostname a.example\n");
        write_config(cfg_dir.join("b.conf"), "Host b\n  Hostname b.example\n");

        // Subdirectory should be ignored by the `is_file()` filter.
        fs::create_dir_all(cfg_dir.join("nested")).unwrap();

        let pattern = format!("{}/*", cfg_dir.display());
        let result = expand_config_paths(&[pattern]);

        assert_eq!(result.len(), 2);
        let names: Vec<String> = result
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"a.conf".to_string()));
        assert!(names.contains(&"b.conf".to_string()));
    }

    #[test]
    fn expand_config_paths_empty_glob_yields_no_paths() {
        let dir = tempdir();
        let pattern = format!("{}/missing-dir/*", dir.display());
        let result = expand_config_paths(&[pattern]);
        assert!(result.is_empty());
    }

    #[test]
    fn parse_config_collects_hosts_from_glob_directory() {
        // Reproduces the "~/.ssh/config + ~/.ssh/config.d/*" scenario without
        // touching the real ~/.ssh layout.
        let dir = tempdir();
        let main_cfg = dir.join("config");
        let cfg_d = dir.join("config.d");
        fs::create_dir_all(&cfg_d).unwrap();

        write_config(&main_cfg, "Host main\n  Hostname main.example\n  User root\n");
        write_config(
            cfg_d.join("a.conf"),
            "Host alpha\n  Hostname a.example\n  User a\n",
        );
        write_config(
            cfg_d.join("b.conf"),
            "Host beta\n  Hostname b.example\n  User b\n  ProxyJump alpha\n",
        );

        let paths = expand_config_paths(&[
            main_cfg.to_string_lossy().into_owned(),
            format!("{}/*", cfg_d.display()),
        ]);

        let mut hosts: Vec<Host> = Vec::new();
        for p in &paths {
            hosts.extend(parse_config(p).expect("parse"));
        }

        let names: Vec<&str> = hosts.iter().map(|h| h.name.as_str()).collect();
        assert!(names.contains(&"main"));
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
        // ProxyJump should be captured for use in the detail panel.
        let beta = hosts.iter().find(|h| h.name == "beta").unwrap();
        assert_eq!(beta.proxy_jump.as_deref(), Some("alpha"));
    }

    /// Minimal stand-in for `tempfile::tempdir()` so we do not add a new
    /// dev-dependency just for these tests. Cleanup is best-effort; the OS
    /// will reclaim the directory eventually.
    fn tempdir() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("sgh-test-{pid}-{n}"));
        fs::create_dir_all(&path).expect("create tempdir");
        path
    }
}
