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

//! Package lima handles generating Lima configuration files and managing
//! VM lifecycle (create, start, stop, delete, status) on remote hosts.

use crate::config::NodeConfig;
use crate::logging;
use crate::remote::{Context, Runner};

/// VMStatus represents the state of a Lima VM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VMStatus {
    Running,
    Stopped,
    NotCreated,
    Unknown(String),
}

impl VMStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Running => "Running",
            Self::Stopped => "Stopped",
            Self::NotCreated => "NotCreated",
            Self::Unknown(s) => {
                if s.is_empty() {
                    "Unknown"
                } else {
                    s.as_str()
                }
            }
        }
    }

    pub fn from_status_str(s: &str) -> Self {
        match s {
            "Running" => Self::Running,
            "Stopped" => Self::Stopped,
            "NotCreated" => Self::NotCreated,
            other => Self::Unknown(other.to_string()),
        }
    }
}

impl std::fmt::Display for VMStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Manager handles Lima VM operations on a remote host.
#[derive(Debug, Clone)]
pub struct Manager {
    pub runner: Runner,
    pub node: NodeConfig,
    pub vm_name: String,
}

impl Manager {
    /// Creates a new Lima manager for the given host and node config.
    pub fn new(runner: Runner, node: NodeConfig) -> Self {
        let vm_name = node.get_vm_name().to_string();
        Self {
            runner,
            node,
            vm_name,
        }
    }

    /// Returns the current VM status on the remote host.
    pub fn status(&self, ctx: &Context) -> Result<VMStatus, String> {
        let out = match self.runner.run(ctx, "limactl list --json") {
            Ok(o) => o,
            Err(err) => {
                if err.contains("command not found") {
                    return Err(format!("lima is not installed on {}", self.runner.host));
                }
                return Err(err);
            }
        };

        for raw_line in out.lines() {
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }
            if let (Some(name), Some(status)) = (
                extract_json_string_field(line, "name"),
                extract_json_string_field(line, "status"),
            ) {
                if name == self.vm_name {
                    return Ok(VMStatus::from_status_str(&status));
                }
            }
        }

        Ok(VMStatus::NotCreated)
    }

    /// Generates the Lima YAML config for this node.
    pub fn generate_config(&self, socket_path: &str) -> String {
        format!(
            r#"vmType: "vz"
images:
  - location: "https://cloud-images.ubuntu.com/releases/24.04/release/ubuntu-24.04-server-cloudimg-arm64.img"
    arch: "aarch64"
  - location: "https://cloud-images.ubuntu.com/releases/24.04/release/ubuntu-24.04-server-cloudimg-amd64.img"
    arch: "x86_64"
cpus: {}
memory: "{}"
disk: "{}"
networks:
  - socket: "{}"
provision:
  - mode: system
    script: |
      #!/bin/bash
      apt-get update -qq && apt-get install -y -qq curl open-iscsi nfs-common
"#,
            self.node.vm.cpus, self.node.vm.memory, self.node.vm.disk, socket_path
        )
    }

    /// Creates and starts the Lima VM on the remote host.
    pub fn provision(&self, ctx: &Context) -> Result<(), String> {
        let status = self.status(ctx)?;

        match status {
            VMStatus::Running => {
                logging::debug(
                    "VM already running, skipping provision",
                    &[("host", &self.runner.host), ("vm", &self.vm_name)],
                );
                println!(
                    "  [{}] VM already running, skipping provision",
                    self.runner.host
                );
                Ok(())
            }
            VMStatus::Stopped => {
                logging::info(
                    "VM stopped, starting",
                    &[("host", &self.runner.host), ("vm", &self.vm_name)],
                );
                println!(
                    "  [{}] VM exists but stopped, starting...",
                    self.runner.host
                );
                self.runner
                    .run(ctx, &format!("limactl start {}", self.vm_name))?;
                Ok(())
            }
            VMStatus::NotCreated => {
                logging::info(
                    "creating and starting VM",
                    &[
                        ("host", &self.runner.host),
                        ("vm", &self.vm_name),
                        ("cpus", &self.node.vm.cpus.to_string()),
                        ("memory", &self.node.vm.memory),
                        ("disk", &self.node.vm.disk),
                    ],
                );
                println!("  [{}] Creating and starting VM...", self.runner.host);

                let socket_path = self
                    .detect_socket_path(ctx)
                    .map_err(|e| format!("detecting socket_vmnet path: {}", e))?;

                let lima_config = self.generate_config(&socket_path);
                let config_path = format!("~/{}.yaml", self.vm_name);
                let encoded = base64_encode(lima_config.as_bytes());
                self.runner
                    .run(
                        ctx,
                        &format!("echo {} | base64 -d > {}", encoded, config_path),
                    )
                    .map_err(|e| format!("writing lima config: {}", e))?;

                let start_cmd = format!(
                    "limactl start --name={} {} --tty=false --timeout=15m0s",
                    self.vm_name, config_path
                );
                if let Err(err) = self.runner.run(ctx, &start_cmd) {
                    if let Ok(VMStatus::Running) = self.status(ctx) {
                        logging::warn(
                            "limactl start returned error but VM is running",
                            &[("host", &self.runner.host)],
                        );
                    } else {
                        return Err(format!("starting lima VM: {}", err));
                    }
                }

                Ok(())
            }
            other => Err(format!("unexpected VM status: {}", other)),
        }
    }

    /// Returns the bridged LAN IP address of the VM.
    pub fn get_bridged_ip(&self, ctx: &Context) -> Result<String, String> {
        let cmd1 =
            "ip -4 addr show lima0 2>/dev/null | awk '/inet / {split($2,a,\"/\"); print a[1]}'";
        if let Ok(out) = self.runner.lima_shell(ctx, &self.vm_name, cmd1) {
            if !out.trim().is_empty() {
                return Ok(out.trim().to_string());
            }
        }

        let cmd2 =
            "ip -4 route get 1.1.1.1 | awk '/src/ {for(i=1;i<=NF;i++) if($i==\"src\") print $(i+1)}'";
        let out = self
            .runner
            .lima_shell(ctx, &self.vm_name, cmd2)
            .map_err(|e| format!("could not determine bridged IP: {}", e))?;

        Ok(out.trim().to_string())
    }

    /// Stops and deletes the Lima VM.
    pub fn destroy(&self, ctx: &Context) -> Result<(), String> {
        let status = self.status(ctx)?;
        if status == VMStatus::NotCreated {
            logging::debug(
                "no VM found, skipping destroy",
                &[("host", &self.runner.host), ("vm", &self.vm_name)],
            );
            println!("  [{}] No VM found, skipping", self.runner.host);
            return Ok(());
        }

        logging::info(
            "destroying VM",
            &[("host", &self.runner.host), ("vm", &self.vm_name)],
        );
        println!("  [{}] Destroying VM...", self.runner.host);
        self.runner.run(
            ctx,
            &format!(
                "limactl stop {} --force 2>/dev/null; limactl delete {} --force",
                self.vm_name, self.vm_name
            ),
        )?;
        Ok(())
    }

    /// Finds the actual `socket_vmnet` socket on the remote host.
    pub fn detect_socket_path(&self, ctx: &Context) -> Result<String, String> {
        let candidates = [
            "/opt/homebrew/var/run/socket_vmnet",
            "/usr/local/var/run/socket_vmnet",
            "/var/run/socket_vmnet",
        ];

        for path in candidates {
            if self
                .runner
                .run_shell(ctx, &format!("sudo test -S {}", path))
                .is_ok()
            {
                logging::debug(
                    "found socket_vmnet",
                    &[("host", &self.runner.host), ("path", path)],
                );
                return Ok(path.to_string());
            }
        }

        if let Ok(out) = self.runner.run_shell(ctx, "brew --prefix 2>/dev/null") {
            let prefix = out.trim();
            let path = format!("{}/var/run/socket_vmnet", prefix);
            logging::debug(
                "using brew prefix for socket_vmnet",
                &[("host", &self.runner.host), ("path", &path)],
            );
            return Ok(path);
        }

        Err(format!(
            "[{}] could not find socket_vmnet socket — ensure socket_vmnet is installed and running",
            self.runner.host
        ))
    }
}

/// Encodes a byte slice to standard Base64 without external dependencies.
pub fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Extracts a top-level string field `"<key>":"<value>"` from a single-line JSON object.
pub fn extract_json_string_field(json_line: &str, key: &str) -> Option<String> {
    let trimmed = json_line.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let pattern = format!("\"{}\"", key);
    let idx = trimmed.find(&pattern)?;
    let after_key = trimmed[idx + pattern.len()..].trim_start();
    let after_colon = after_key.strip_prefix(':')?.trim_start();
    let after_quote = after_colon.strip_prefix('"')?;
    let mut result = String::new();
    let mut escaped = false;
    for ch in after_quote.chars() {
        if escaped {
            result.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(result);
        } else {
            result.push(ch);
        }
    }
    None
}
