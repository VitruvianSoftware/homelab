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

//! Package doctor provides pre-flight and health checks for the homelab cluster.

use std::collections::BTreeMap;

use crate::config::{Config, NodeConfig};
use crate::k3s;
use crate::lima::{self, VMStatus};
use crate::remote::{Context, Runner};

/// CheckResult represents the outcome of a single diagnostic check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub name: String,
    pub host: String,
    pub passed: bool,
    pub message: String,
}

/// Executes all diagnostic checks and reports the results.
pub fn run(ctx: &Context, cfg: &Config) -> Result<(), String> {
    run_with_runner_factory(ctx, cfg, |host| Runner::new(host))
}

/// Executes all diagnostic checks using a customizable runner factory (enables unit testing).
pub fn run_with_runner_factory<F>(ctx: &Context, cfg: &Config, make_runner: F) -> Result<(), String>
where
    F: Fn(&str) -> Runner,
{
    println!("🩺 Running diagnostics for cluster: {}", cfg.cluster.name);
    println!();

    let results = collect_diagnostics(ctx, cfg, &make_runner);
    let mut failed = 0usize;

    println!();
    for r in &results {
        let icon = if r.passed {
            "✅"
        } else {
            failed += 1;
            "❌"
        };
        println!("  {} [{:<25}] {}: {}", icon, r.host, r.name, r.message);
    }

    println!();
    let total = results.len();
    let passed = total - failed;
    println!("  Results: {}/{} checks passed", passed, total);

    if failed > 0 {
        return Err(format!("{} check(s) failed", failed));
    }

    println!("\n  🎉 All checks passed!");
    Ok(())
}

/// Runs all diagnostic checks and returns the ordered list of `CheckResult`s.
pub fn collect_diagnostics<F>(ctx: &Context, cfg: &Config, make_runner: F) -> Vec<CheckResult>
where
    F: Fn(&str) -> Runner,
{
    let mut results = Vec::new();

    for node in &cfg.nodes {
        let runner = make_runner(&node.host);

        results.push(check_ssh(ctx, &runner, &node.host));
        results.push(check_brew(ctx, &runner, &node.host));
        results.push(check_lima(ctx, &runner, &node.host));
        results.push(check_socket_vmnet(ctx, &runner, &node.host));

        let lima_mgr = lima::Manager::new(runner.clone(), node.clone());
        results.push(check_vm(ctx, &lima_mgr, &node.host));
        results.push(check_bridged_ip(ctx, &lima_mgr, &node.host));

        let k3s_mgr = k3s::Manager::new(runner);
        results.push(check_k3s(ctx, &k3s_mgr, &node.host));
    }

    let ips = collect_bridged_ips(ctx, cfg, &make_runner);
    if ips.len() > 1 {
        for (host, ip) in &ips {
            for (other_host, other_ip) in &ips {
                if host == other_host {
                    continue;
                }
                let runner = make_runner(host);
                let lima_mgr = lima::Manager::new(
                    runner.clone(),
                    NodeConfig {
                        host: host.clone(),
                        ..NodeConfig::default()
                    },
                );
                results.push(check_ping(
                    ctx, &runner, &lima_mgr, host, other_host, other_ip, ip,
                ));
            }
        }
    }

    let init_node = cfg.init_node();
    let init_runner = make_runner(&init_node.host);
    let init_k3s = k3s::Manager::new(init_runner);
    results.push(check_cluster_health(ctx, &init_k3s, &init_node.host));

    results
}

pub fn check_ssh(ctx: &Context, runner: &Runner, host: &str) -> CheckResult {
    match runner.run(ctx, "echo ok") {
        Ok(_) => CheckResult {
            name: "SSH".to_string(),
            host: host.to_string(),
            passed: true,
            message: "connected".to_string(),
        },
        Err(err) => CheckResult {
            name: "SSH".to_string(),
            host: host.to_string(),
            passed: false,
            message: format!("connection failed: {}", err),
        },
    }
}

pub fn check_brew(ctx: &Context, runner: &Runner, host: &str) -> CheckResult {
    match runner.run(ctx, "which brew || ls /opt/homebrew/bin/brew") {
        Ok(_) => CheckResult {
            name: "Homebrew".to_string(),
            host: host.to_string(),
            passed: true,
            message: "installed".to_string(),
        },
        Err(_) => CheckResult {
            name: "Homebrew".to_string(),
            host: host.to_string(),
            passed: false,
            message: "not found".to_string(),
        },
    }
}

pub fn check_lima(ctx: &Context, runner: &Runner, host: &str) -> CheckResult {
    match runner.run(ctx, "limactl --version 2>/dev/null || echo not-found") {
        Ok(out) if !out.contains("not-found") => CheckResult {
            name: "Lima".to_string(),
            host: host.to_string(),
            passed: true,
            message: out.trim().to_string(),
        },
        _ => CheckResult {
            name: "Lima".to_string(),
            host: host.to_string(),
            passed: false,
            message: "not installed".to_string(),
        },
    }
}

pub fn check_socket_vmnet(ctx: &Context, runner: &Runner, host: &str) -> CheckResult {
    let cmd = "if sudo test -S /opt/homebrew/var/run/socket_vmnet; then echo /opt/homebrew/var/run/socket_vmnet; elif sudo test -S /usr/local/var/run/socket_vmnet; then echo /usr/local/var/run/socket_vmnet; elif sudo test -S /var/run/socket_vmnet; then echo /var/run/socket_vmnet; else exit 1; fi";
    match runner.run_shell(ctx, cmd) {
        Ok(out) => CheckResult {
            name: "socket_vmnet".to_string(),
            host: host.to_string(),
            passed: true,
            message: format!("running at {}", out.trim()),
        },
        Err(_) => CheckResult {
            name: "socket_vmnet".to_string(),
            host: host.to_string(),
            passed: false,
            message: "socket not found".to_string(),
        },
    }
}

pub fn check_vm(ctx: &Context, mgr: &lima::Manager, host: &str) -> CheckResult {
    match mgr.status(ctx) {
        Ok(status) => CheckResult {
            name: "Lima VM".to_string(),
            host: host.to_string(),
            passed: status == VMStatus::Running,
            message: status.to_string(),
        },
        Err(err) => CheckResult {
            name: "Lima VM".to_string(),
            host: host.to_string(),
            passed: false,
            message: format!("error: {}", err),
        },
    }
}

pub fn check_bridged_ip(ctx: &Context, mgr: &lima::Manager, host: &str) -> CheckResult {
    match mgr.get_bridged_ip(ctx) {
        Ok(ip) => CheckResult {
            name: "Bridged IP".to_string(),
            host: host.to_string(),
            passed: true,
            message: ip,
        },
        Err(err) => CheckResult {
            name: "Bridged IP".to_string(),
            host: host.to_string(),
            passed: false,
            message: format!("not available: {}", err),
        },
    }
}

pub fn check_k3s(ctx: &Context, mgr: &k3s::Manager, host: &str) -> CheckResult {
    match mgr.is_installed(ctx) {
        Ok(true) => CheckResult {
            name: "K3s".to_string(),
            host: host.to_string(),
            passed: true,
            message: "installed".to_string(),
        },
        Ok(false) => CheckResult {
            name: "K3s".to_string(),
            host: host.to_string(),
            passed: false,
            message: "not installed".to_string(),
        },
        Err(err) => CheckResult {
            name: "K3s".to_string(),
            host: host.to_string(),
            passed: false,
            message: format!("error: {}", err),
        },
    }
}

pub fn check_ping(
    ctx: &Context,
    runner: &Runner,
    _mgr: &lima::Manager,
    from_host: &str,
    to_host: &str,
    to_ip: &str,
    _from_ip: &str,
) -> CheckResult {
    let name = format!("Ping {}", to_host);
    match runner.lima_shell(ctx, "k8s-node", &format!("ping -c 1 -W 3 {}", to_ip)) {
        Ok(_) => CheckResult {
            name,
            host: from_host.to_string(),
            passed: true,
            message: format!("reachable ({})", to_ip),
        },
        Err(_) => CheckResult {
            name,
            host: from_host.to_string(),
            passed: false,
            message: format!("cannot reach {} ({})", to_host, to_ip),
        },
    }
}

pub fn check_cluster_health(ctx: &Context, mgr: &k3s::Manager, host: &str) -> CheckResult {
    match mgr.get_node_status(ctx) {
        Ok(out) => {
            if out.contains("NotReady") {
                CheckResult {
                    name: "Cluster Health".to_string(),
                    host: host.to_string(),
                    passed: false,
                    message: "some nodes are NotReady".to_string(),
                }
            } else {
                CheckResult {
                    name: "Cluster Health".to_string(),
                    host: host.to_string(),
                    passed: true,
                    message: "all nodes Ready".to_string(),
                }
            }
        }
        Err(_) => CheckResult {
            name: "Cluster Health".to_string(),
            host: host.to_string(),
            passed: false,
            message: "API server not reachable".to_string(),
        },
    }
}

fn collect_bridged_ips<F>(ctx: &Context, cfg: &Config, make_runner: &F) -> BTreeMap<String, String>
where
    F: Fn(&str) -> Runner,
{
    let mut ips = BTreeMap::new();
    for node in &cfg.nodes {
        let runner = make_runner(&node.host);
        let mgr = lima::Manager::new(runner, node.clone());
        if let Ok(ip) = mgr.get_bridged_ip(ctx) {
            ips.insert(node.host.clone(), ip);
        }
    }
    ips
}
