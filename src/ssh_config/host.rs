use regex::Regex;
use std::collections::HashMap;

use super::EntryType;

#[derive(Debug, serde::Serialize, Clone)]
pub struct LocalForward {
    pub local_port: String,
    pub remote_host: String,
    pub remote_port: String,
}

pub(crate) type Entry = (EntryType, String);

#[derive(Debug, Clone)]
pub struct Host {
    patterns: Vec<String>,
    entries: HashMap<EntryType, String>,

    pub local_forwards: Vec<LocalForward>,
}

impl Host {
    #[must_use]
    pub fn new(patterns: Vec<String>) -> Host {
        Host {
            patterns,
            entries: HashMap::new(),
            local_forwards: vec![],
        }
    }

    /// SSH Configの各行(key-value)を更新する
    pub fn update(&mut self, entry: Entry) {
        match entry.0 {
            EntryType::LocalForward => {
                // 例: value = "8888 localhost:8888"
                let trimmed = entry.1.trim();
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 2 {
                    let local_port = parts[0];
                    let remote_info = parts[1];
                    let remote_parts: Vec<&str> = remote_info.split(':').collect();
                    if remote_parts.len() == 2 {
                        let lf = LocalForward {
                            local_port: local_port.to_string(),
                            remote_host: remote_parts[0].to_string(),
                            remote_port: remote_parts[1].to_string(),
                        };
                        self.local_forwards.push(lf);
                    }
                }
            }
            _ => {
                self.entries.insert(entry.0, entry.1);
            }
        }
    }

    pub(crate) fn extend_patterns(&mut self, host: &Host) {
        self.patterns.extend(host.patterns.clone());
    }

    pub(crate) fn extend_entries(&mut self, host: &Host) {
        self.entries.extend(host.entries.clone());
        self.local_forwards.extend(host.local_forwards.clone());
    }

    pub(crate) fn extend_if_not_contained(&mut self, host: &Host) {
        for (key, value) in &host.entries {
            if !self.entries.contains_key(key) {
                self.entries.insert(key.clone(), value.clone());
            }
        }
        for lf in &host.local_forwards {
            self.local_forwards.push(lf.clone());
        }
    }

    #[allow(clippy::must_use_candidate)]
    pub fn get_patterns(&self) -> &Vec<String> {
        &self.patterns
    }

    /// # Panics
    ///
    /// Will panic if the regex cannot be compiled.
    #[allow(clippy::must_use_candidate)]
    pub fn matching_pattern_regexes(&self) -> Vec<(Regex, bool)> {
        if self.patterns.is_empty() {
            return Vec::new();
        }

        self.patterns
            .iter()
            .filter_map(|pattern| {
                let contains_wildcard =
                    pattern.contains('*') || pattern.contains('?') || pattern.contains('!');
                if !contains_wildcard {
                    return None;
                }

                let mut pattern = pattern
                    .replace('.', r"\.")
                    .replace('*', ".*")
                    .replace('?', ".");

                let is_negated = pattern.starts_with('!');
                if is_negated {
                    pattern.remove(0);
                }

                pattern = format!("^{pattern}$");
                Some((Regex::new(&pattern).unwrap(), is_negated))
            })
            .collect()
    }

    #[allow(clippy::must_use_candidate)]
    pub fn get(&self, entry: &EntryType) -> Option<String> {
        self.entries.get(entry).cloned()
    }

    #[allow(clippy::must_use_candidate)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.local_forwards.is_empty()
    }
}

#[allow(clippy::module_name_repetitions)]
pub trait HostVecExt {
    /// Apply the name entry to the hostname entry if the hostname entry is empty.
    #[must_use]
    fn apply_name_to_empty_hostname(&self) -> Self;

    /// Merges the hosts with the same entries into one host.
    #[must_use]
    fn merge_same_hosts(&self) -> Self;

    /// Spreads the hosts with multiple patterns into multiple hosts with one pattern.
    #[must_use]
    fn spread(&self) -> Self;

    /// Apply patterns entries to non-pattern hosts and remove the pattern hosts.
    #[must_use]
    fn apply_patterns(&self) -> Self;
}

impl HostVecExt for Vec<Host> {
    fn apply_name_to_empty_hostname(&self) -> Self {
        let mut hosts = self.clone();

        for host in &mut hosts {
            if host.get(&EntryType::Hostname).is_none() {
                if let Some(name) = host.patterns.first() {
                    host.update((EntryType::Hostname, name.clone()));
                }
            }
        }

        hosts
    }

    fn merge_same_hosts(&self) -> Self {
        let mut hosts = self.clone();

        for i in (0..hosts.len()).rev() {
            let (left, right) = hosts.split_at_mut(i);

            let current_host = &right[0];

            for j in (0..i).rev() {
                let target_host = &mut left[j];

                // entries が同じかどうかで判定しているが、
                // LocalForward の違いもチェックしたいなら custom で書く
                if current_host.entries != target_host.entries {
                    continue;
                }

                // if we want to compare local_forwards as well

                // if current_host.local_forwards != target_host.local_forwards {
                //     continue;
                // }

                if current_host
                    .entries
                    .values()
                    .any(|value| value.contains("%h"))
                {
                    continue;
                }

                target_host.extend_patterns(current_host);
                target_host.extend_entries(current_host);
                hosts.remove(i);
                break;
            }
        }

        hosts
    }

    fn spread(&self) -> Vec<Host> {
        let mut hosts = Vec::new();

        for host in self {
            let patterns = host.get_patterns();
            if patterns.is_empty() {
                hosts.push(host.clone());
                continue;
            }

            for pattern in patterns {
                let mut new_host = host.clone();
                new_host.patterns = vec![pattern.clone()];
                hosts.push(new_host);
            }
        }

        hosts
    }

    /// Apply patterns entries to non-pattern hosts and remove the pattern hosts.
    fn apply_patterns(&self) -> Self {
        let mut hosts = self.spread();
        let mut pattern_indexes = Vec::new();

        for i in 0..hosts.len() {
            let matching_pattern_regexes = hosts[i].matching_pattern_regexes();
            if matching_pattern_regexes.is_empty() {
                continue;
            }

            pattern_indexes.push(i);

            for j in 0..hosts.len() {
                if i == j {
                    continue;
                }

                if !hosts[j].matching_pattern_regexes().is_empty() {
                    continue;
                }

                for (regex, is_negated) in &matching_pattern_regexes {
                    if regex.is_match(&hosts[j].patterns[0]) == *is_negated {
                        continue;
                    }

                    let host = hosts[i].clone();
                    hosts[j].extend_if_not_contained(&host);
                    break;
                }
            }
        }

        for i in pattern_indexes.into_iter().rev() {
            hosts.remove(i);
        }

        hosts
    }
}
