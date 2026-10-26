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

//! Package k3s handles K3s installation, cluster join, and lifecycle management
//! on remote Lima VMs.

use std::fs;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::logging;
use crate::remote::{go_quote, Context, Runner};

pub const DEFAULT_VM_NAME: &str = "k8s-node";

/// Manager handles K3s operations on a remote Lima VM.
#[derive(Debug, Clone)]
pub struct Manager {
    pub runner: Runner,
    pub vm_name: String,
    pub poll_interval: Duration,
}

impl Manager {
    /// Creates a new K3s manager for the given remote host.
    pub fn new(runner: Runner) -> Self {
        Self {
            runner,
            vm_name: DEFAULT_VM_NAME.to_string(),
            poll_interval: Duration::from_secs(5),
        }
    }

    /// Creates a new K3s manager with a custom VM name.
    pub fn with_vm(runner: Runner, vm_name: &str) -> Self {
        let effective = if vm_name.is_empty() {
            DEFAULT_VM_NAME
        } else {
            vm_name
        };
        Self {
            runner,
            vm_name: effective.to_string(),
            poll_interval: Duration::from_secs(5),
        }
    }

    /// Checks whether K3s is installed inside the VM.
    pub fn is_installed(&self, ctx: &Context) -> Result<bool, String> {
        let out = self.runner.lima_shell_sudo(
            ctx,
            &self.vm_name,
            "test -f /usr/local/bin/k3s && echo yes || echo no",
        )?;
        Ok(out.trim() == "yes")
    }

    /// Builds the shell script used by `init_cluster`.
    pub fn build_init_cluster_script(
        &self,
        node_ip: &str,
        pool: &str,
        k3s_version: &str,
        tls_sans: &[String],
    ) -> String {
        let mut san_flags = String::new();
        for san in tls_sans {
            san_flags.push_str(&format!(" --tls-san={}", san));
        }

        let version_env = if !k3s_version.is_empty() {
            format!("INSTALL_K3S_VERSION={} ", go_quote(k3s_version))
        } else {
            String::new()
        };

        format!(
            "curl -sfL https://get.k3s.io | {}INSTALL_K3S_EXEC=\"server\" sh -s - --cluster-init --node-name={} --node-ip={} --advertise-address={}{} --flannel-iface=lima0 --node-label=pool={}",
            version_env, self.runner.host, node_ip, node_ip, san_flags, pool
        )
    }

    /// Bootstraps the first control plane node with `--cluster-init`.
    pub fn init_cluster(
        &self,
        ctx: &Context,
        node_ip: &str,
        pool: &str,
        k3s_version: &str,
        tls_sans: &[String],
    ) -> Result<(), String> {
        if self.is_installed(ctx)? {
            logging::debug(
                "K3s already installed, skipping init",
                &[("host", &self.runner.host)],
            );
            println!(
                "  [{}] K3s already installed, skipping init",
                self.runner.host
            );
            return Ok(());
        }

        logging::info(
            "initializing K3s cluster",
            &[
                ("host", &self.runner.host),
                ("node_ip", node_ip),
                ("pool", pool),
            ],
        );
        println!("  [{}] Initializing K3s cluster...", self.runner.host);

        let script = self.build_init_cluster_script(node_ip, pool, k3s_version, tls_sans);
        logging::debug(
            "K3s install script",
            &[("host", &self.runner.host), ("script", &script)],
        );
        self.runner.lima_shell_sudo(ctx, &self.vm_name, &script)?;
        Ok(())
    }

    /// Builds the shell script used by `join_server`.
    pub fn build_join_server_script(
        &self,
        node_ip: &str,
        server_url: &str,
        token: &str,
        pool: &str,
        k3s_version: &str,
        tls_sans: &[String],
    ) -> String {
        let mut san_flags = String::new();
        for san in tls_sans {
            san_flags.push_str(&format!(" --tls-san={}", san));
        }

        let version_env = if !k3s_version.is_empty() {
            format!("INSTALL_K3S_VERSION={} ", go_quote(k3s_version))
        } else {
            String::new()
        };

        format!(
            "curl -sfL https://get.k3s.io | {}INSTALL_K3S_SKIP_START=true K3S_TOKEN={} INSTALL_K3S_EXEC=\"server\" sh -s - --server={} --node-name={} --node-ip={} --advertise-address={}{} --flannel-iface=lima0 --node-label=pool={}",
            version_env,
            go_quote(token),
            server_url,
            self.runner.host,
            node_ip,
            node_ip,
            san_flags,
            pool
        )
    }

    /// Joins a server node to an existing HA cluster.
    pub fn join_server(
        &self,
        ctx: &Context,
        node_ip: &str,
        server_url: &str,
        token: &str,
        pool: &str,
        k3s_version: &str,
        tls_sans: &[String],
    ) -> Result<(), String> {
        if self.is_installed(ctx)? {
            logging::debug(
                "K3s already installed, skipping join",
                &[("host", &self.runner.host)],
            );
            println!(
                "  [{}] K3s already installed, skipping join",
                self.runner.host
            );
            return Ok(());
        }

        logging::info(
            "joining cluster as server",
            &[
                ("host", &self.runner.host),
                ("node_ip", node_ip),
                ("server_url", server_url),
            ],
        );
        println!("  [{}] Joining cluster as server...", self.runner.host);

        let script =
            self.build_join_server_script(node_ip, server_url, token, pool, k3s_version, tls_sans);
        logging::debug("K3s join script", &[("host", &self.runner.host)]);
        self.runner.lima_shell_sudo(ctx, &self.vm_name, &script)?;

        println!(
            "  [{}] Starting K3s server (joining cluster)...",
            self.runner.host
        );
        self.runner
            .lima_shell_sudo(ctx, &self.vm_name, "systemctl start k3s --no-block")
            .map_err(|e| format!("[{}] starting k3s service: {}", self.runner.host, e))?;

        self.wait_for_join(ctx, Duration::from_secs(180))
    }

    fn wait_for_join(&self, ctx: &Context, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;

        while Instant::now() < deadline {
            if let Some(ctx_err) = ctx.err() {
                return Err(format!(
                    "[{}] context cancelled while waiting for K3s join: {}",
                    self.runner.host, ctx_err
                ));
            }

            if let Ok(out) = self.runner.lima_shell_sudo(
                ctx,
                &self.vm_name,
                "systemctl is-active k3s 2>/dev/null",
            ) {
                if out.trim() == "active" {
                    logging::info("K3s joined successfully", &[("host", &self.runner.host)]);
                    return Ok(());
                }
            }

            logging::debug(
                "waiting for K3s to join cluster",
                &[("host", &self.runner.host)],
            );
            println!(
                "  [{}] Waiting for K3s to join cluster...",
                self.runner.host
            );
            ctx.sleep(self.poll_interval)
                .map_err(|e| format!("[{}] context cancelled: {}", self.runner.host, e))?;
        }

        Err(format!(
            "[{}] timed out waiting for K3s to join cluster",
            self.runner.host
        ))
    }

    /// Builds the shell script used by `join_agent`.
    pub fn build_join_agent_script(
        &self,
        node_ip: &str,
        server_url: &str,
        token: &str,
        pool: &str,
        k3s_version: &str,
    ) -> String {
        let version_env = if !k3s_version.is_empty() {
            format!("INSTALL_K3S_VERSION={} ", go_quote(k3s_version))
        } else {
            String::new()
        };

        format!(
            "curl -sfL https://get.k3s.io | {}K3S_TOKEN={} K3S_URL={} sh -s - agent --node-name={} --node-ip={} --flannel-iface=lima0 --node-label=pool={}",
            version_env,
            go_quote(token),
            go_quote(server_url),
            self.runner.host,
            node_ip,
            pool
        )
    }

    /// Joins a worker node to the cluster.
    pub fn join_agent(
        &self,
        ctx: &Context,
        node_ip: &str,
        server_url: &str,
        token: &str,
        pool: &str,
        k3s_version: &str,
    ) -> Result<(), String> {
        if self.is_installed(ctx)? {
            logging::debug(
                "K3s already installed, skipping join",
                &[("host", &self.runner.host)],
            );
            println!(
                "  [{}] K3s already installed, skipping join",
                self.runner.host
            );
            return Ok(());
        }

        logging::info(
            "joining cluster as agent",
            &[
                ("host", &self.runner.host),
                ("node_ip", node_ip),
                ("server_url", server_url),
            ],
        );
        println!("  [{}] Joining cluster as agent...", self.runner.host);

        let script = self.build_join_agent_script(node_ip, server_url, token, pool, k3s_version);
        logging::debug("K3s agent join script", &[("host", &self.runner.host)]);
        self.runner.lima_shell_sudo(ctx, &self.vm_name, &script)?;
        Ok(())
    }

    /// Retrieves the K3s node token from the server.
    pub fn get_token(&self, ctx: &Context) -> Result<String, String> {
        let out = self
            .runner
            .lima_shell_sudo(
                ctx,
                &self.vm_name,
                "cat /var/lib/rancher/k3s/server/node-token",
            )
            .map_err(|e| format!("[{}] reading node token: {}", self.runner.host, e))?;
        Ok(out.trim().to_string())
    }

    /// Polls until K3s is initialized, the node token exists, and the API server responds.
    pub fn wait_for_ready(&self, ctx: &Context, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;

        while Instant::now() < deadline {
            if let Some(ctx_err) = ctx.err() {
                return Err(format!(
                    "[{}] context cancelled while waiting for K3s: {}",
                    self.runner.host, ctx_err
                ));
            }

            let out = self.runner.lima_shell_sudo(
                ctx,
                &self.vm_name,
                "test -f /var/lib/rancher/k3s/server/node-token && echo ready || echo waiting",
            );
            if out.as_deref().map(str::trim) != Ok("ready") {
                logging::debug("waiting for K3s node-token", &[("host", &self.runner.host)]);
                println!("  [{}] Waiting for K3s to initialize...", self.runner.host);
                ctx.sleep(self.poll_interval).map_err(|e| {
                    format!(
                        "[{}] context cancelled while waiting for K3s: {}",
                        self.runner.host, e
                    )
                })?;
                continue;
            }

            if self
                .runner
                .lima_shell_sudo(
                    ctx,
                    &self.vm_name,
                    "k3s kubectl get nodes --request-timeout=5s 2>/dev/null",
                )
                .is_ok()
            {
                logging::info("K3s is ready", &[("host", &self.runner.host)]);
                return Ok(());
            }

            logging::debug(
                "K3s token exists but API server not ready yet",
                &[("host", &self.runner.host)],
            );
            println!(
                "  [{}] K3s initializing (API server starting)...",
                self.runner.host
            );
            ctx.sleep(self.poll_interval).map_err(|e| {
                format!(
                    "[{}] context cancelled while waiting for K3s: {}",
                    self.runner.host, e
                )
            })?;
        }

        Err(format!(
            "[{}] timed out waiting for K3s to become ready",
            self.runner.host
        ))
    }

    /// Returns the output of `kubectl get nodes`.
    pub fn get_node_status(&self, ctx: &Context) -> Result<String, String> {
        self.runner.lima_shell_sudo(
            ctx,
            &self.vm_name,
            "k3s kubectl get nodes -o wide -L pool -L kubernetes.io/arch",
        )
    }

    /// Removes K3s from the VM.
    pub fn uninstall(&self, ctx: &Context, role: &str) -> Result<(), String> {
        if !self.is_installed(ctx)? {
            logging::debug(
                "K3s not installed, skipping uninstall",
                &[("host", &self.runner.host)],
            );
            return Ok(());
        }

        logging::info(
            "uninstalling K3s",
            &[("host", &self.runner.host), ("role", role)],
        );
        println!("  [{}] Uninstalling K3s...", self.runner.host);

        let script = if role == "agent" {
            "/usr/local/bin/k3s-agent-uninstall.sh"
        } else {
            "/usr/local/bin/k3s-uninstall.sh"
        };

        self.runner.lima_shell_sudo(ctx, &self.vm_name, script)?;
        Ok(())
    }

    /// Drains a node before removal.
    pub fn drain_node(&self, ctx: &Context, node_name: &str) -> Result<(), String> {
        logging::info(
            "draining node",
            &[("host", &self.runner.host), ("node", node_name)],
        );
        println!("  [{}] Draining node {}...", self.runner.host, node_name);
        self.runner.lima_shell_sudo(
            ctx,
            &self.vm_name,
            &format!(
                "k3s kubectl drain {} --ignore-daemonsets --delete-emptydir-data --timeout=60s",
                node_name
            ),
        )?;
        Ok(())
    }

    /// Removes a node from the cluster.
    pub fn delete_node(&self, ctx: &Context, node_name: &str) -> Result<(), String> {
        logging::info(
            "deleting node from cluster",
            &[("host", &self.runner.host), ("node", node_name)],
        );
        println!(
            "  [{}] Deleting node {} from cluster...",
            self.runner.host, node_name
        );
        self.runner.lima_shell_sudo(
            ctx,
            &self.vm_name,
            &format!("k3s kubectl delete node {}", node_name),
        )?;
        Ok(())
    }

    /// Retrieves and patches the kubeconfig for external access.
    pub fn get_kubeconfig(&self, ctx: &Context, server_ip: &str) -> Result<String, String> {
        let out =
            self.runner
                .lima_shell_sudo(ctx, &self.vm_name, "cat /etc/rancher/k3s/k3s.yaml")?;
        Ok(out.replace("127.0.0.1", server_ip))
    }

    // --- Advanced operations (from internal/k3s/advanced.go) ---

    /// Returns the installed K3s version string.
    pub fn get_version(&self, ctx: &Context) -> Result<String, String> {
        let out = self.runner.lima_shell_sudo(
            ctx,
            &self.vm_name,
            "k3s --version 2>/dev/null | head -1",
        )?;
        let parts: Vec<&str> = out.split_whitespace().collect();
        if parts.len() >= 3 {
            return Ok(parts[2].to_string());
        }
        Ok(out.trim().to_string())
    }

    /// Builds the script for `reinstall_server`.
    pub fn build_reinstall_server_script(
        &self,
        node_ip: &str,
        version: &str,
        tls_sans: &[String],
        pool: &str,
    ) -> String {
        let mut san_flags = String::new();
        for san in tls_sans {
            san_flags.push_str(&format!(" --tls-san={}", san));
        }

        format!(
            "curl -sfL https://get.k3s.io | INSTALL_K3S_VERSION={} INSTALL_K3S_EXEC=\"server\" sh -s - --node-ip={} --advertise-address={}{} --flannel-iface=lima0 --node-label=pool={}",
            go_quote(version),
            node_ip,
            node_ip,
            san_flags,
            pool
        )
    }

    /// Reinstalls K3s server (init node) with a new version.
    pub fn reinstall_server(
        &self,
        ctx: &Context,
        node_ip: &str,
        version: &str,
        tls_sans: &[String],
        pool: &str,
    ) -> Result<(), String> {
        let script = self.build_reinstall_server_script(node_ip, version, tls_sans, pool);
        logging::debug(
            "K3s reinstall server script",
            &[("host", &self.runner.host)],
        );
        self.runner.lima_shell_sudo(ctx, &self.vm_name, &script)?;
        Ok(())
    }

    /// Builds the script for `reinstall_join_server`.
    pub fn build_reinstall_join_server_script(
        &self,
        node_ip: &str,
        server_url: &str,
        token: &str,
        version: &str,
        pool: &str,
    ) -> String {
        format!(
            "curl -sfL https://get.k3s.io | INSTALL_K3S_VERSION={} K3S_TOKEN={} INSTALL_K3S_EXEC=\"server\" sh -s - --server={} --node-ip={} --advertise-address={} --tls-san={} --flannel-iface=lima0 --node-label=pool={}",
            go_quote(version),
            go_quote(token),
            server_url,
            node_ip,
            node_ip,
            node_ip,
            pool
        )
    }

    /// Reinstalls K3s on a server node that joins an existing cluster.
    pub fn reinstall_join_server(
        &self,
        ctx: &Context,
        node_ip: &str,
        server_url: &str,
        token: &str,
        version: &str,
        pool: &str,
    ) -> Result<(), String> {
        let script =
            self.build_reinstall_join_server_script(node_ip, server_url, token, version, pool);
        logging::debug(
            "K3s reinstall join-server script",
            &[("host", &self.runner.host)],
        );
        self.runner.lima_shell_sudo(ctx, &self.vm_name, &script)?;
        Ok(())
    }

    /// Builds the script for `reinstall_agent`.
    pub fn build_reinstall_agent_script(
        &self,
        node_ip: &str,
        server_url: &str,
        token: &str,
        version: &str,
        pool: &str,
    ) -> String {
        format!(
            "curl -sfL https://get.k3s.io | INSTALL_K3S_VERSION={} K3S_TOKEN={} K3S_URL={} sh -s - agent --node-ip={} --flannel-iface=lima0 --node-label=pool={}",
            go_quote(version),
            go_quote(token),
            go_quote(server_url),
            node_ip,
            pool
        )
    }

    /// Reinstalls K3s on an agent node.
    pub fn reinstall_agent(
        &self,
        ctx: &Context,
        node_ip: &str,
        server_url: &str,
        token: &str,
        version: &str,
        pool: &str,
    ) -> Result<(), String> {
        let script = self.build_reinstall_agent_script(node_ip, server_url, token, version, pool);
        logging::debug("K3s reinstall agent script", &[("host", &self.runner.host)]);
        self.runner.lima_shell_sudo(ctx, &self.vm_name, &script)?;
        Ok(())
    }

    /// Marks a node as schedulable after draining.
    pub fn uncordon_node(&self, ctx: &Context, node_name: &str) -> Result<(), String> {
        logging::info(
            "uncordoning node",
            &[("host", &self.runner.host), ("node", node_name)],
        );
        self.runner.lima_shell_sudo(
            ctx,
            &self.vm_name,
            &format!("k3s kubectl uncordon {}", node_name),
        )?;
        Ok(())
    }

    /// Creates an etcd snapshot and returns the snapshot path.
    pub fn create_snapshot(&self, ctx: &Context) -> Result<String, String> {
        let timestamp = format_utc_timestamp();
        let snapshot_name = format!("homelab-snapshot-{}", timestamp);

        logging::info(
            "creating etcd snapshot",
            &[("host", &self.runner.host), ("name", &snapshot_name)],
        );
        self.runner.lima_shell_sudo(
            ctx,
            &self.vm_name,
            &format!("k3s etcd-snapshot save --name {}", snapshot_name),
        )?;

        Ok(format!(
            "/var/lib/rancher/k3s/server/db/snapshots/{}",
            snapshot_name
        ))
    }

    /// Copies a snapshot from the remote VM to the local machine.
    pub fn download_snapshot(
        &self,
        ctx: &Context,
        remote_path: &str,
        local_path: &str,
    ) -> Result<(), String> {
        logging::info(
            "downloading snapshot",
            &[
                ("host", &self.runner.host),
                ("remote", remote_path),
                ("local", local_path),
            ],
        );

        let content = self
            .runner
            .lima_shell_sudo(ctx, &self.vm_name, &format!("base64 {}", remote_path))
            .map_err(|e| format!("reading snapshot: {}", e))?;

        let target = format!("{}.b64", local_path);
        fs::write(&target, content.as_bytes()).map_err(|e| format!("writing snapshot: {}", e))?;

        Ok(())
    }

    /// Copies a snapshot from the local machine to the remote VM.
    pub fn upload_snapshot(
        &self,
        ctx: &Context,
        local_path: &str,
        remote_path: &str,
    ) -> Result<(), String> {
        logging::info(
            "uploading snapshot",
            &[
                ("host", &self.runner.host),
                ("local", local_path),
                ("remote", remote_path),
            ],
        );

        let content =
            fs::read_to_string(local_path).map_err(|e| format!("reading local snapshot: {}", e))?;

        self.runner.lima_shell_sudo(
            ctx,
            &self.vm_name,
            &format!(
                "cat > {} << 'SNAPSHOTEOF'\n{}\nSNAPSHOTEOF",
                remote_path, content
            ),
        )?;
        Ok(())
    }

    /// Restores etcd from a snapshot file on the remote VM.
    pub fn restore_snapshot(&self, ctx: &Context, snapshot_path: &str) -> Result<(), String> {
        logging::info(
            "restoring etcd from snapshot",
            &[("host", &self.runner.host), ("snapshot", snapshot_path)],
        );
        self.runner.lima_shell_sudo(
            ctx,
            &self.vm_name,
            &format!(
                "k3s server --cluster-reset --cluster-reset-restore-path={}",
                snapshot_path
            ),
        )?;
        Ok(())
    }

    /// Stops the K3s systemd service.
    pub fn stop_service(&self, ctx: &Context) -> Result<(), String> {
        logging::info("stopping K3s service", &[("host", &self.runner.host)]);
        self.runner
            .lima_shell_sudo(ctx, &self.vm_name, "systemctl stop k3s")?;
        Ok(())
    }

    /// Starts the K3s systemd service.
    pub fn start_service(&self, ctx: &Context) -> Result<(), String> {
        logging::info("starting K3s service", &[("host", &self.runner.host)]);
        self.runner
            .lima_shell_sudo(ctx, &self.vm_name, "systemctl start k3s")?;
        Ok(())
    }
}

/// Formats the current UTC time as `YYYYMMDD-HHMMSS` using Howard Hinnant's civil-from-days algorithm.
pub fn format_utc_timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format_unix_timestamp(secs)
}

/// Formats a Unix epoch second value as `YYYYMMDD-HHMMSS` in UTC.
pub fn format_unix_timestamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        year, m, d, hour, minute, second
    )
}
