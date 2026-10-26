// Copyright (c) 2026 VitruvianSoftware
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Package config handles loading, parsing, and validating the homelab cluster
//! configuration from YAML files.

use std::collections::HashSet;
use std::env;
use std::fs;

/// Config is the top-level configuration for a homelab cluster.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub cluster: ClusterConfig,
    pub nodes: Vec<NodeConfig>,
}

/// ClusterConfig holds cluster-wide settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClusterConfig {
    pub name: String,
    pub k3s_version: String,
    pub kubeconfig: String,
}

/// NodeConfig describes a single node in the cluster.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeConfig {
    pub host: String,
    pub role: String, // "server" or "agent"
    pub pool: String,
    pub vm: VMConfig,
    pub vm_name: String,      // Override default VM name (default: "k8s-node")
    pub ssh_user: String,     // SSH username (default: current user)
    pub ssh_port: String,     // SSH port (default: 22)
    pub ssh_key_path: String, // Path to SSH private key (optional)
}

/// VMConfig describes the resource allocation for a Lima VM.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VMConfig {
    pub cpus: i32,
    pub memory: String,
    pub disk: String,
}

impl NodeConfig {
    /// Returns the VM name for this node, defaulting to `"k8s-node"`.
    pub fn get_vm_name(&self) -> &str {
        if !self.vm_name.is_empty() {
            &self.vm_name
        } else {
            "k8s-node"
        }
    }
}

impl Config {
    /// Reads, parses, and validates a config file from the given path.
    /// Supports `HOMELAB_CONFIG` environment variable as a fallback when `path == "homelab.yaml"`.
    pub fn load(path: &str) -> Result<Config, String> {
        let mut effective_path = path.to_string();
        if path == "homelab.yaml" {
            if let Ok(env_path) = env::var("HOMELAB_CONFIG") {
                if !env_path.is_empty() {
                    effective_path = env_path;
                }
            }
        }

        let data = fs::read_to_string(&effective_path)
            .map_err(|e| format!("reading config file {}: {}", effective_path, e))?;

        let cfg = Self::from_yaml(&data)
            .map_err(|e| format!("parsing config file {}: {}", effective_path, e))?;

        cfg.validate()
            .map_err(|e| format!("validating config: {}", e))?;

        Ok(cfg)
    }

    /// Parses a YAML string into a `Config` without running validation.
    pub fn from_yaml(content: &str) -> Result<Config, String> {
        let mut cfg = Config::default();

        #[derive(PartialEq, Eq)]
        enum Section {
            None,
            Cluster,
            Nodes,
        }

        let mut section = Section::None;
        let mut in_node_vm = false;
        let mut current_node: Option<NodeConfig> = None;

        for (line_idx, raw_line) in content.lines().enumerate() {
            let line_num = line_idx + 1;
            let without_comment = strip_yaml_comment(raw_line);
            let trimmed = without_comment.trim();
            if trimmed.is_empty() || trimmed == "---" {
                continue;
            }

            if trimmed.starts_with('{') || trimmed.starts_with('[') || trimmed.contains("{{") {
                return Err(format!("line {}: malformed YAML syntax", line_num));
            }

            let indent = without_comment.len() - without_comment.trim_start().len();

            if indent == 0 {
                if in_node_vm {
                    in_node_vm = false;
                }
                if let Some(node) = current_node.take() {
                    cfg.nodes.push(node);
                }

                let (key, val) = split_key_value(trimmed, line_num)?;
                match key {
                    "cluster" => {
                        if !val.is_empty() {
                            return Err(format!("line {}: cluster must be a mapping", line_num));
                        }
                        section = Section::Cluster;
                    }
                    "nodes" => {
                        if !val.is_empty() && val != "[]" {
                            return Err(format!("line {}: nodes must be a sequence", line_num));
                        }
                        section = Section::Nodes;
                    }
                    other => {
                        return Err(format!(
                            "line {}: unknown top-level key {:?}",
                            line_num, other
                        ));
                    }
                }
                continue;
            }

            match section {
                Section::None => {
                    return Err(format!(
                        "line {}: unexpected indented line before section",
                        line_num
                    ));
                }
                Section::Cluster => {
                    let (key, val) = split_key_value(trimmed, line_num)?;
                    let clean_val = unquote_yaml_value(val, line_num)?;
                    match key {
                        "name" => cfg.cluster.name = clean_val,
                        "k3sVersion" => cfg.cluster.k3s_version = clean_val,
                        "kubeconfig" => cfg.cluster.kubeconfig = clean_val,
                        _ => {}
                    }
                }
                Section::Nodes => {
                    if let Some(after_dash) = trimmed.strip_prefix('-') {
                        in_node_vm = false;
                        if let Some(node) = current_node.take() {
                            cfg.nodes.push(node);
                        }
                        let mut node = NodeConfig::default();
                        let rest = after_dash.trim();
                        if !rest.is_empty() {
                            let (key, val) = split_key_value(rest, line_num)?;
                            if key == "vm" {
                                in_node_vm = true;
                            } else {
                                apply_node_field(&mut node, key, val, line_num)?;
                            }
                        }
                        current_node = Some(node);
                    } else {
                        let node = current_node.as_mut().ok_or_else(|| {
                            format!("line {}: node property outside of list item", line_num)
                        })?;
                        let (key, val) = split_key_value(trimmed, line_num)?;
                        if key == "vm" && val.is_empty() {
                            in_node_vm = true;
                        } else if in_node_vm && matches!(key, "cpus" | "memory" | "disk") {
                            apply_vm_field(&mut node.vm, key, val, line_num)?;
                        } else {
                            in_node_vm = false;
                            apply_node_field(node, key, val, line_num)?;
                        }
                    }
                }
            }
        }

        if let Some(node) = current_node.take() {
            cfg.nodes.push(node);
        }

        Ok(cfg)
    }

    /// Checks that the config is well-formed and internally consistent.
    pub fn validate(&self) -> Result<(), String> {
        let mut errs: Vec<String> = Vec::new();

        if self.cluster.name.is_empty() {
            errs.push("cluster.name is required".to_string());
        }

        if self.nodes.is_empty() {
            errs.push("at least one node must be defined".to_string());
        }

        let servers = self.server_nodes();
        if servers.is_empty() {
            errs.push("at least one server node is required".to_string());
        }

        if servers.len() > 1 && servers.len() % 2 == 0 {
            errs.push(format!(
                "HA requires an odd number of server nodes for etcd quorum, got {}",
                servers.len()
            ));
        }

        let mut hosts = HashSet::new();
        let mut pools = HashSet::new();

        for (i, n) in self.nodes.iter().enumerate() {
            if n.host.is_empty() {
                errs.push(format!("nodes[{}].host is required", i));
            }
            if n.role != "server" && n.role != "agent" {
                errs.push(format!(
                    "nodes[{}].role must be 'server' or 'agent', got {:?}",
                    i, n.role
                ));
            }
            if n.pool.is_empty() {
                errs.push(format!("nodes[{}].pool is required", i));
            }
            if n.vm.cpus < 1 {
                errs.push(format!("nodes[{}].vm.cpus must be >= 1", i));
            }
            if n.vm.memory.is_empty() {
                errs.push(format!("nodes[{}].vm.memory is required", i));
            }
            if n.vm.disk.is_empty() {
                errs.push(format!("nodes[{}].vm.disk is required", i));
            }

            if !hosts.insert(n.host.clone()) {
                errs.push(format!("duplicate host: {:?}", n.host));
            }

            if !pools.insert(n.pool.clone()) {
                errs.push(format!("duplicate pool label: {:?}", n.pool));
            }
        }

        if !errs.is_empty() {
            return Err(format!(
                "config validation failed:\n  - {}",
                errs.join("\n  - ")
            ));
        }

        Ok(())
    }

    /// Returns only the nodes with role `"server"`.
    pub fn server_nodes(&self) -> Vec<NodeConfig> {
        self.nodes
            .iter()
            .filter(|n| n.role == "server")
            .cloned()
            .collect()
    }

    /// Returns only the nodes with role `"agent"`.
    pub fn agent_nodes(&self) -> Vec<NodeConfig> {
        self.nodes
            .iter()
            .filter(|n| n.role == "agent")
            .cloned()
            .collect()
    }

    /// Returns the first server node, which is used to bootstrap the cluster.
    pub fn init_node(&self) -> NodeConfig {
        self.server_nodes()
            .into_iter()
            .next()
            .expect("config must have at least one server node")
    }
}

fn strip_yaml_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    for (idx, ch) in line.char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => {
                if idx == 0 || line.as_bytes()[idx - 1].is_ascii_whitespace() {
                    return &line[..idx];
                }
            }
            _ => {}
        }
    }
    line
}

fn split_key_value(s: &str, line_num: usize) -> Result<(&str, &str), String> {
    let colon = s
        .find(':')
        .ok_or_else(|| format!("line {}: expected key: value pair, got {:?}", line_num, s))?;
    let key = s[..colon].trim();
    if key.is_empty() || key.contains(' ') {
        return Err(format!("line {}: invalid YAML key {:?}", line_num, key));
    }
    let val = s[colon + 1..].trim();
    Ok((key, val))
}

fn unquote_yaml_value(val: &str, line_num: usize) -> Result<String, String> {
    if val.is_empty() {
        return Ok(String::new());
    }
    if (val.starts_with('"') && !val.ends_with('"'))
        || (val.starts_with('\'') && !val.ends_with('\''))
        || ((val.starts_with('"') || val.starts_with('\'')) && val.len() == 1)
    {
        return Err(format!(
            "line {}: unclosed quote in value {:?}",
            line_num, val
        ));
    }
    if (val.starts_with('"') && val.ends_with('"'))
        || (val.starts_with('\'') && val.ends_with('\''))
    {
        return Ok(val[1..val.len() - 1].to_string());
    }
    Ok(val.to_string())
}

fn apply_node_field(
    node: &mut NodeConfig,
    key: &str,
    val: &str,
    line_num: usize,
) -> Result<(), String> {
    let clean = unquote_yaml_value(val, line_num)?;
    match key {
        "host" => node.host = clean,
        "role" => node.role = clean,
        "pool" => node.pool = clean,
        "vmName" => node.vm_name = clean,
        "sshUser" => node.ssh_user = clean,
        "sshPort" => node.ssh_port = clean,
        "sshKeyPath" => node.ssh_key_path = clean,
        _ => {}
    }
    Ok(())
}

fn apply_vm_field(vm: &mut VMConfig, key: &str, val: &str, line_num: usize) -> Result<(), String> {
    let clean = unquote_yaml_value(val, line_num)?;
    match key {
        "cpus" => {
            vm.cpus = clean
                .parse::<i32>()
                .map_err(|_| format!("line {}: invalid integer for cpus: {:?}", line_num, clean))?;
        }
        "memory" => vm.memory = clean,
        "disk" => vm.disk = clean,
        _ => {}
    }
    Ok(())
}
