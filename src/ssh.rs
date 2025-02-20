use anyhow::anyhow;
use handlebars::Handlebars;
use itertools::Itertools;
use serde::Serialize;
use ssh_config::host::LocalForward;
use std::collections::VecDeque;
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

/// # Errors
///
/// Will return `Err` if the SSH configuration file cannot be parsed.
pub fn parse_config(raw_path: &String) -> Result<Vec<Host>, ParseConfigError> {
    let normalized_path = shellexpand::tilde(raw_path).to_string();
    let path = std::fs::canonicalize(normalized_path)?;

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
            local_forwards: h.local_forwards.clone(),
        })
        .collect();

    Ok(hosts)
}
