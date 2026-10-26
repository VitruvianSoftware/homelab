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

//! Package cluster orchestrates the full lifecycle of a homelab K8s cluster.
//! It coordinates Lima VM provisioning, K3s installation, and cluster operations
//! across multiple remote macOS hosts.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::config::{Config, NodeConfig};
use crate::k3s;
use crate::lima::{self, VMStatus};
use crate::logging;
use crate::prereqs;
use crate::remote::{Context, Runner};

/// InitOptions configures the `init` operation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InitOptions {
    pub dry_run: bool,
    pub auto_install: bool,
}

/// Bootstraps a new cluster from scratch. Idempotent.
pub fn init(ctx: &Context, cfg: &Config, opts: InitOptions) -> Result<(), String> {
    init_with_runner_factory(ctx, cfg, opts, new_runner)
}

/// Bootstraps a new cluster using a provided runner factory.
pub fn init_with_runner_factory<F>(
    ctx: &Context,
    cfg: &Config,
    opts: InitOptions,
    make_runner: F,
) -> Result<(), String>
where
    F: Fn(&NodeConfig) -> Runner + Send + Sync + Clone + 'static,
{
    logging::info(
        "initializing homelab cluster",
        &[
            ("name", &cfg.cluster.name),
            ("nodes", &cfg.nodes.len().to_string()),
            ("dry_run", &opts.dry_run.to_string()),
        ],
    );

    if opts.dry_run {
        print_dry_run_plan(cfg);
        return Ok(());
    }

    // Phase 0: Ensure prerequisites on all hosts (parallel).
    println!("🔍 Phase 0: Checking prerequisites...");
    {
        let mut handles = Vec::new();
        for node in cfg.nodes.clone() {
            let ctx_clone = ctx.clone();
            let factory = make_runner.clone();
            let auto_install = opts.auto_install;
            handles.push(thread::spawn(move || {
                let runner = factory(&node);
                prereqs::ensure_all_with_runner(&ctx_clone, &runner, &node.host, auto_install)
            }));
        }
        for h in handles {
            let res = h
                .join()
                .map_err(|_| "prerequisite thread panicked".to_string())?;
            if let Err(err) = res {
                return Err(format!("prerequisite check failed: {}", err));
            }
        }
    }

    // Phase 1: Provision all Lima VMs (parallel).
    println!("\n📦 Phase 1: Provisioning VMs (parallel)...");
    let ip_map: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
    {
        let total = cfg.nodes.len();
        let mut handles = Vec::new();
        for (i, node) in cfg.nodes.iter().cloned().enumerate() {
            let idx = i + 1;
            let ctx_clone = ctx.clone();
            let factory = make_runner.clone();
            let ip_map_clone = Arc::clone(&ip_map);
            handles.push(thread::spawn(move || -> Result<(), String> {
                let runner = factory(&node);
                let mgr = lima::Manager::new(runner, node.clone());

                mgr.provision(&ctx_clone)
                    .map_err(|e| format!("[{}] provisioning VM: {}", node.host, e))?;

                let ip = mgr
                    .get_bridged_ip(&ctx_clone)
                    .map_err(|e| format!("[{}] getting bridged IP: {}", node.host, e))?;

                ip_map_clone
                    .lock()
                    .unwrap()
                    .insert(node.host.clone(), ip.clone());

                println!("  [{}/{}] {}: VM ready (IP: {})", idx, total, node.host, ip);
                logging::info("VM provisioned", &[("host", &node.host), ("ip", &ip)]);
                Ok(())
            }));
        }
        for h in handles {
            let res = h
                .join()
                .map_err(|_| "provisioning thread panicked".to_string())?;
            if let Err(err) = res {
                return Err(format!(
                    "VM provisioning failed: {}\n\nRecovery: re-run 'homelab init' to retry",
                    err
                ));
            }
        }
    }

    let ip_map = Arc::try_unwrap(ip_map)
        .map_err(|_| "ip_map still referenced".to_string())?
        .into_inner()
        .unwrap();

    // Phase 2: Validate cross-VM network connectivity.
    println!("\n🌐 Phase 2: Validating network connectivity...");
    validate_network(ctx, cfg, &ip_map, &make_runner)?;

    let mut server_ips = Vec::new();
    for node in cfg.server_nodes() {
        if let Some(ip) = ip_map.get(&node.host) {
            server_ips.push(ip.clone());
        }
    }

    // Phase 3: Initialize the first control plane node.
    println!("\n🔧 Phase 3: Bootstrapping control plane (CP-1)...");
    let init_node = cfg.init_node();
    let init_runner = make_runner(&init_node);
    let init_k3s = k3s::Manager::new(init_runner);
    let init_ip = ip_map.get(&init_node.host).cloned().unwrap_or_default();

    init_k3s
        .init_cluster(
            ctx,
            &init_ip,
            &init_node.pool,
            &cfg.cluster.k3s_version,
            &server_ips,
        )
        .map_err(|e| {
            format!(
                "[{}] initializing K3s: {}\n\nRecovery: re-run 'homelab init' to retry",
                init_node.host, e
            )
        })?;

    init_k3s.wait_for_ready(ctx, Duration::from_secs(300))?;

    let token = init_k3s
        .get_token(ctx)
        .map_err(|e| format!("[{}] getting token: {}", init_node.host, e))?;
    logging::info(
        "cluster initialized, token acquired",
        &[("host", &init_node.host)],
    );

    let server_url = format!("https://{}:6443", init_ip);

    // Phase 4: Join remaining server nodes.
    let servers = cfg.server_nodes();
    if servers.len() > 1 {
        println!("\n🔗 Phase 4: Joining control plane nodes...");
        for node in &servers[1..] {
            let runner = make_runner(node);
            let k3s_mgr = k3s::Manager::new(runner);
            let node_ip = ip_map.get(&node.host).cloned().unwrap_or_default();

            k3s_mgr
                .join_server(
                    ctx,
                    &node_ip,
                    &server_url,
                    &token,
                    &node.pool,
                    &cfg.cluster.k3s_version,
                    std::slice::from_ref(&node_ip),
                )
                .map_err(|e| {
                    format!(
                        "[{}] joining as server: {}\n\nRecovery: re-run 'homelab init' to retry",
                        node.host, e
                    )
                })?;

            k3s_mgr.wait_for_ready(ctx, Duration::from_secs(300))?;
            logging::info("node joined as server", &[("host", &node.host)]);
        }
    }

    // Phase 5: Join agent nodes.
    let agents = cfg.agent_nodes();
    if !agents.is_empty() {
        println!("\n🔗 Phase 5: Joining worker nodes...");
        for node in &agents {
            let runner = make_runner(node);
            let k3s_mgr = k3s::Manager::new(runner);
            let node_ip = ip_map.get(&node.host).cloned().unwrap_or_default();

            k3s_mgr
                .join_agent(
                    ctx,
                    &node_ip,
                    &server_url,
                    &token,
                    &node.pool,
                    &cfg.cluster.k3s_version,
                )
                .map_err(|e| {
                    format!(
                        "[{}] joining as agent: {}\n\nRecovery: re-run 'homelab init' to retry",
                        node.host, e
                    )
                })?;
            logging::info("node joined as agent", &[("host", &node.host)]);
        }
    }

    // Phase 6: Export kubeconfig.
    println!("\n📋 Phase 6: Exporting kubeconfig...");
    export_kubeconfig(ctx, &init_k3s, &init_ip, &cfg.cluster.kubeconfig)?;

    // Phase 7: Final validation.
    println!("\n✅ Phase 7: Validating cluster...");
    let out = init_k3s
        .get_node_status(ctx)
        .map_err(|e| format!("[{}] getting node status: {}", init_node.host, e))?;
    println!("{}", out);
    println!("\n🎉 Cluster {:?} is ready!", cfg.cluster.name);

    Ok(())
}

/// Iterates over all nodes in the cluster, gracefully rolling updates to VM configurations.
pub fn apply(ctx: &Context, cfg: &Config, dry_run: bool) -> Result<(), String> {
    apply_with_runner_factory(ctx, cfg, dry_run, Duration::from_secs(10), new_runner)
}

/// Applies configuration changes with a customizable runner factory and startup settle delay.
pub fn apply_with_runner_factory<F>(
    ctx: &Context,
    cfg: &Config,
    dry_run: bool,
    settle_delay: Duration,
    make_runner: F,
) -> Result<(), String>
where
    F: Fn(&NodeConfig) -> Runner,
{
    logging::info(
        "applying rolling updates to cluster",
        &[
            ("name", &cfg.cluster.name),
            ("dry_run", &dry_run.to_string()),
        ],
    );

    let init_node = cfg.init_node();
    let init_runner = make_runner(&init_node);
    let init_k3s = k3s::Manager::new(init_runner);

    init_k3s
        .wait_for_ready(ctx, Duration::from_secs(30))
        .map_err(|e| format!("control plane not reachable on {}: {}", init_node.host, e))?;

    for node in &cfg.nodes {
        let runner = make_runner(node);
        let lima_mgr = lima::Manager::new(runner.clone(), node.clone());

        match lima_mgr.status(ctx) {
            Ok(VMStatus::NotCreated) | Err(_) => {
                logging::info("skipping node (not provisioned)", &[("host", &node.host)]);
                continue;
            }
            Ok(_) => {}
        }

        let out = runner
            .run_shell(ctx, &format!("limactl list {} --json", node.get_vm_name()))
            .map_err(|e| format!("[{}] fetching lima properties: {}", node.host, e))?;

        let expected_cpus = format!("\"cpus\":{}", node.vm.cpus);
        if out.contains(&expected_cpus) {
            logging::debug(
                "node resources match configuration",
                &[("host", &node.host)],
            );
            continue;
        }

        logging::info(
            "node hardware config differs from requested config",
            &[
                ("host", &node.host),
                ("cpus", &node.vm.cpus.to_string()),
                ("memory", &node.vm.memory),
            ],
        );
        println!("\n🔄 Applying update to {}...", node.host);
        if dry_run {
            println!(
                "  [DRY RUN] Would drain {}, stop VM, update CPUs/Memory to {}/{}, and restart.",
                node.host, node.vm.cpus, node.vm.memory
            );
            continue;
        }

        println!("  [{}] Draining Kubernetes node...", node.host);
        if let Err(err) = init_k3s.drain_node(ctx, &node.host) {
            logging::warn(
                "drain failed, but proceeding",
                &[("host", &node.host), ("error", &err)],
            );
        }

        println!("  [{}] Stopping VM...", node.host);
        runner
            .run_shell(ctx, &format!("limactl stop {}", node.get_vm_name()))
            .map_err(|e| format!("[{}] stopping VM: {}", node.host, e))?;

        println!(
            "  [{}] Applying new hardware limits (CPU={}, Memory={})...",
            node.host, node.vm.cpus, node.vm.memory
        );
        let sed_cmd = format!(
            "sed -i.bak -e 's/^cpus: .*/cpus: {}/' -e 's/^memory: .*/memory: \"{}\"/' ~/.lima/{}/lima.yaml",
            node.vm.cpus,
            node.vm.memory,
            node.get_vm_name()
        );
        runner
            .run_shell(ctx, &sed_cmd)
            .map_err(|e| format!("[{}] updating lima.yaml: {}", node.host, e))?;

        println!("  [{}] Restarting VM...", node.host);
        runner
            .run_shell(ctx, &format!("limactl start {}", node.get_vm_name()))
            .map_err(|e| format!("[{}] starting VM: {}", node.host, e))?;

        println!(
            "  [{}] Waiting for Kubernetes node to become ready...",
            node.host
        );
        let _ = ctx.sleep(settle_delay);

        println!("  [{}] Uncordoning node...", node.host);
        init_k3s
            .uncordon_node(ctx, &node.host)
            .map_err(|e| format!("[{}] uncordoning node: {}", node.host, e))?;

        println!("  ✅ [{}] Update applied successfully.", node.host);
    }

    println!("\n🎉 All nodes are up to date.");
    Ok(())
}

/// Adds nodes that are in the config but not yet in the cluster.
pub fn join(ctx: &Context, cfg: &Config, dry_run: bool) -> Result<(), String> {
    join_with_runner_factory(ctx, cfg, dry_run, new_runner)
}

/// Adds missing nodes to the cluster using a provided runner factory.
pub fn join_with_runner_factory<F>(
    ctx: &Context,
    cfg: &Config,
    dry_run: bool,
    make_runner: F,
) -> Result<(), String>
where
    F: Fn(&NodeConfig) -> Runner,
{
    logging::info(
        "joining new nodes to cluster",
        &[
            ("name", &cfg.cluster.name),
            ("dry_run", &dry_run.to_string()),
        ],
    );

    let mut init_ip = String::new();
    let mut token = String::new();
    let mut found = false;

    for n in cfg.server_nodes() {
        let runner = make_runner(&n);
        let k3s_mgr = k3s::Manager::new(runner.clone());

        if !k3s_mgr.is_installed(ctx).unwrap_or(false) {
            continue;
        }

        let lima_mgr = lima::Manager::new(runner, n.clone());
        let ip = match lima_mgr.get_bridged_ip(ctx) {
            Ok(ip) => ip,
            Err(err) => {
                logging::debug(
                    "failed to get IP for healthy server",
                    &[("host", &n.host), ("error", &err)],
                );
                continue;
            }
        };

        let t = match k3s_mgr.get_token(ctx) {
            Ok(t) => t,
            Err(err) => {
                logging::debug(
                    "failed to get token from healthy server",
                    &[("host", &n.host), ("error", &err)],
                );
                continue;
            }
        };

        init_ip = ip;
        token = t;
        found = true;
        break;
    }

    if !found {
        return Err("no existing healthy server node found to join against".to_string());
    }

    let server_url = format!("https://{}:6443", init_ip);

    for node in &cfg.nodes {
        let runner = make_runner(node);
        let k3s_mgr = k3s::Manager::new(runner.clone());
        let lima_mgr = lima::Manager::new(runner, node.clone());

        if k3s_mgr.is_installed(ctx).unwrap_or(false) {
            logging::debug(
                "node already part of cluster, skipping",
                &[("host", &node.host)],
            );
            continue;
        }

        if dry_run {
            println!(
                "  [{}] Would join as {} (pool={})",
                node.host, node.role, node.pool
            );
            continue;
        }

        lima_mgr
            .provision(ctx)
            .map_err(|e| format!("[{}] provisioning: {}", node.host, e))?;

        let node_ip = lima_mgr
            .get_bridged_ip(ctx)
            .map_err(|e| format!("[{}] getting IP: {}", node.host, e))?;

        match node.role.as_str() {
            "server" => {
                k3s_mgr
                    .join_server(
                        ctx,
                        &node_ip,
                        &server_url,
                        &token,
                        &node.pool,
                        &cfg.cluster.k3s_version,
                        std::slice::from_ref(&node_ip),
                    )
                    .map_err(|e| format!("[{}] joining as server: {}", node.host, e))?;
            }
            "agent" => {
                k3s_mgr
                    .join_agent(
                        ctx,
                        &node_ip,
                        &server_url,
                        &token,
                        &node.pool,
                        &cfg.cluster.k3s_version,
                    )
                    .map_err(|e| format!("[{}] joining as agent: {}", node.host, e))?;
            }
            _ => {}
        }

        logging::info("node joined", &[("host", &node.host), ("role", &node.role)]);
    }

    Ok(())
}

/// Drains and removes a specific node from the cluster.
pub fn remove(ctx: &Context, cfg: &Config, host_name: &str, dry_run: bool) -> Result<(), String> {
    remove_with_runner_factory(ctx, cfg, host_name, dry_run, new_runner)
}

/// Drains and removes a specific node using a provided runner factory.
pub fn remove_with_runner_factory<F>(
    ctx: &Context,
    cfg: &Config,
    host_name: &str,
    dry_run: bool,
    make_runner: F,
) -> Result<(), String>
where
    F: Fn(&NodeConfig) -> Runner,
{
    logging::info(
        "removing node",
        &[("host", host_name), ("dry_run", &dry_run.to_string())],
    );

    let target = cfg
        .nodes
        .iter()
        .find(|n| n.host == host_name)
        .cloned()
        .ok_or_else(|| format!("node {:?} not found in config", host_name))?;

    if dry_run {
        println!(
            "  Would drain and remove {} ({}) from cluster",
            host_name, target.role
        );
        return Ok(());
    }

    let init_node = cfg.init_node();
    let init_runner = make_runner(&init_node);
    let init_k3s = k3s::Manager::new(init_runner);

    if let Err(err) = init_k3s.drain_node(ctx, host_name) {
        logging::warn(
            "drain failed (may already be removed)",
            &[("host", host_name), ("error", &err)],
        );
    }

    if let Err(err) = init_k3s.delete_node(ctx, host_name) {
        logging::warn(
            "delete failed (may already be removed)",
            &[("host", host_name), ("error", &err)],
        );
    }

    let target_runner = make_runner(&target);
    let target_k3s = k3s::Manager::new(target_runner);
    target_k3s
        .uninstall(ctx, &target.role)
        .map_err(|e| format!("[{}] uninstalling K3s: {}", target.host, e))?;

    logging::info("node removed successfully", &[("host", host_name)]);
    Ok(())
}

/// Tears down the entire cluster.
pub fn destroy(ctx: &Context, cfg: &Config, force: bool, dry_run: bool) -> Result<(), String> {
    destroy_with_runner_factory(ctx, cfg, force, dry_run, new_runner)
}

/// Tears down the entire cluster using a provided runner factory.
pub fn destroy_with_runner_factory<F>(
    ctx: &Context,
    cfg: &Config,
    force: bool,
    dry_run: bool,
    make_runner: F,
) -> Result<(), String>
where
    F: Fn(&NodeConfig) -> Runner,
{
    logging::info(
        "destroying cluster",
        &[
            ("name", &cfg.cluster.name),
            ("force", &force.to_string()),
            ("dry_run", &dry_run.to_string()),
        ],
    );

    if dry_run {
        println!("Would destroy cluster {:?}:", cfg.cluster.name);
        for node in &cfg.nodes {
            println!(
                "  [{}] Would uninstall K3s ({}) and destroy VM",
                node.host, node.role
            );
        }
        return Ok(());
    }

    if !force {
        print!(
            "⚠️  This will destroy the entire cluster {:?}. Are you sure? [y/N] ",
            cfg.cluster.name
        );
        let _ = io::stdout().flush();
        let mut confirm = String::new();
        let stdin = io::stdin();
        if stdin.lock().read_line(&mut confirm).is_err() || confirm.trim().to_lowercase() != "y" {
            println!("Aborted.");
            return Ok(());
        }
    }

    println!("💥 Destroying cluster: {}", cfg.cluster.name);

    let mut all_nodes = cfg.agent_nodes();
    all_nodes.extend(reverse_nodes(&cfg.server_nodes()));

    for node in &all_nodes {
        let runner = make_runner(node);
        let k3s_mgr = k3s::Manager::new(runner.clone());
        let lima_mgr = lima::Manager::new(runner, node.clone());

        if let Err(err) = k3s_mgr.uninstall(ctx, &node.role) {
            logging::warn(
                "K3s uninstall failed",
                &[("host", &node.host), ("error", &err)],
            );
        }

        if let Err(err) = lima_mgr.destroy(ctx) {
            logging::warn(
                "VM destroy failed",
                &[("host", &node.host), ("error", &err)],
            );
        }
    }

    let kubeconfig_path = expand_path(&cfg.cluster.kubeconfig);
    if !kubeconfig_path.is_empty() {
        match fs::remove_file(&kubeconfig_path) {
            Ok(()) => logging::info("removed kubeconfig", &[("path", &kubeconfig_path)]),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => logging::warn(
                "failed to remove kubeconfig",
                &[("path", &kubeconfig_path), ("error", &e.to_string())],
            ),
        }
    }

    println!("\n🗑️  Cluster {:?} destroyed.", cfg.cluster.name);
    Ok(())
}

/// Displays the current state of all configured nodes.
pub fn status(ctx: &Context, cfg: &Config) -> Result<(), String> {
    status_with_runner_factory(ctx, cfg, new_runner)
}

/// Displays the current state of all configured nodes using a provided runner factory.
pub fn status_with_runner_factory<F>(
    ctx: &Context,
    cfg: &Config,
    make_runner: F,
) -> Result<(), String>
where
    F: Fn(&NodeConfig) -> Runner,
{
    logging::info("checking cluster status", &[("name", &cfg.cluster.name)]);

    println!("📊 Cluster status: {}", cfg.cluster.name);
    println!();

    for node in &cfg.nodes {
        let runner = make_runner(node);
        let lima_mgr = lima::Manager::new(runner.clone(), node.clone());
        let k3s_mgr = k3s::Manager::new(runner);

        let vm_status = lima_mgr.status(ctx).unwrap_or(VMStatus::NotCreated);
        let k3s_installed = k3s_mgr.is_installed(ctx).unwrap_or(false);

        let k3s_status = if k3s_installed {
            "installed"
        } else {
            "not installed"
        };

        println!(
            "  {:<25} role={:<6} pool={:<15} vm={:<12} k3s={}",
            node.host,
            node.role,
            node.pool,
            vm_status.as_str(),
            k3s_status
        );
    }

    println!();
    let init_node = cfg.init_node();
    let init_runner = make_runner(&init_node);
    let init_k3s = k3s::Manager::new(init_runner);
    match init_k3s.get_node_status(ctx) {
        Ok(out) => println!("{}", out),
        Err(_) => println!("  (Cluster not reachable for node status)"),
    }

    Ok(())
}

/// Performs a rolling upgrade of K3s across all nodes.
/// Agents are upgraded first, then servers one at a time to maintain quorum.
pub fn upgrade(ctx: &Context, cfg: &Config, dry_run: bool) -> Result<(), String> {
    upgrade_with_runner_factory(ctx, cfg, dry_run, Duration::from_secs(5), new_runner)
}

/// Performs a rolling upgrade using a provided runner factory and settle delay.
pub fn upgrade_with_runner_factory<F>(
    ctx: &Context,
    cfg: &Config,
    dry_run: bool,
    settle_delay: Duration,
    make_runner: F,
) -> Result<(), String>
where
    F: Fn(&NodeConfig) -> Runner,
{
    let target_version = &cfg.cluster.k3s_version;
    if target_version.is_empty() {
        return Err("cluster.k3sVersion is not set in config".to_string());
    }

    logging::info(
        "starting rolling upgrade",
        &[
            ("target_version", target_version),
            ("dry_run", &dry_run.to_string()),
        ],
    );
    println!("🔄 Rolling upgrade to K3s {}", target_version);

    let init_node = cfg.init_node();
    let init_runner = make_runner(&init_node);
    let init_k3s = k3s::Manager::new(init_runner);

    let agents = cfg.agent_nodes();
    if !agents.is_empty() {
        println!("\n📦 Phase 1: Upgrading agent nodes...");
        for node in &agents {
            upgrade_node(
                ctx,
                cfg,
                &init_k3s,
                node,
                target_version,
                "agent",
                dry_run,
                settle_delay,
                &make_runner,
            )?;
        }
    }

    let servers = cfg.server_nodes();
    if servers.len() > 1 {
        println!("\n📦 Phase 2: Upgrading non-init server nodes...");
        for node in &servers[1..] {
            upgrade_node(
                ctx,
                cfg,
                &init_k3s,
                node,
                target_version,
                "server",
                dry_run,
                settle_delay,
                &make_runner,
            )?;
        }
    }

    println!("\n📦 Phase 3: Upgrading init server node...");
    upgrade_node(
        ctx,
        cfg,
        &init_k3s,
        &init_node,
        target_version,
        "server",
        dry_run,
        settle_delay,
        &make_runner,
    )?;

    if !dry_run {
        println!("\n✅ Phase 4: Validating cluster...");
        let out = init_k3s
            .get_node_status(ctx)
            .map_err(|e| format!("[{}] getting node status: {}", init_node.host, e))?;
        println!("{}", out);
    }

    println!("\n🎉 Upgrade to K3s {} complete!", target_version);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn upgrade_node<F>(
    ctx: &Context,
    cfg: &Config,
    init_k3s: &k3s::Manager,
    node: &NodeConfig,
    version: &str,
    role: &str,
    dry_run: bool,
    settle_delay: Duration,
    make_runner: &F,
) -> Result<(), String>
where
    F: Fn(&NodeConfig) -> Runner,
{
    let runner = make_runner(node);
    let k3s_mgr = k3s::Manager::new(runner.clone());

    let current_version = match k3s_mgr.get_version(ctx) {
        Ok(v) => v,
        Err(err) => {
            logging::warn(
                "could not determine current K3s version",
                &[("host", &node.host), ("error", &err)],
            );
            String::new()
        }
    };

    if current_version == version {
        println!("  [{}] Already at {}, skipping", node.host, version);
        return Ok(());
    }

    if dry_run {
        println!(
            "  [{}] Would upgrade {} → {}",
            node.host, current_version, version
        );
        return Ok(());
    }

    logging::info("draining node for upgrade", &[("host", &node.host)]);
    println!("  [{}] Draining...", node.host);
    if let Err(err) = init_k3s.drain_node(ctx, &node.host) {
        logging::warn(
            "drain failed, continuing",
            &[("host", &node.host), ("error", &err)],
        );
    }

    logging::info(
        "upgrading K3s",
        &[
            ("host", &node.host),
            ("from", &current_version),
            ("to", version),
        ],
    );
    println!(
        "  [{}] Upgrading K3s {} → {}...",
        node.host, current_version, version
    );

    let lima_mgr = lima::Manager::new(runner, node.clone());
    let node_ip = lima_mgr
        .get_bridged_ip(ctx)
        .map_err(|e| format!("[{}] getting IP: {}", node.host, e))?;

    let install_res = match role {
        "server" => {
            if node.host == cfg.init_node().host {
                let mut server_ips = Vec::new();
                for s in cfg.server_nodes() {
                    let s_runner = make_runner(&s);
                    let s_lima = lima::Manager::new(s_runner, s);
                    if let Ok(ip) = s_lima.get_bridged_ip(ctx) {
                        if !ip.is_empty() {
                            server_ips.push(ip);
                        }
                    }
                }
                k3s_mgr.reinstall_server(ctx, &node_ip, version, &server_ips, &node.pool)
            } else {
                let server_url = format!("https://{}:6443", get_init_ip(ctx, cfg, make_runner));
                let token = init_k3s.get_token(ctx).unwrap_or_default();
                k3s_mgr.reinstall_join_server(
                    ctx,
                    &node_ip,
                    &server_url,
                    &token,
                    version,
                    &node.pool,
                )
            }
        }
        "agent" => {
            let server_url = format!("https://{}:6443", get_init_ip(ctx, cfg, make_runner));
            let token = init_k3s.get_token(ctx).unwrap_or_default();
            k3s_mgr.reinstall_agent(ctx, &node_ip, &server_url, &token, version, &node.pool)
        }
        _ => Ok(()),
    };

    install_res.map_err(|e| format!("[{}] upgrade failed: {}", node.host, e))?;

    if role == "server" {
        k3s_mgr.wait_for_ready(ctx, Duration::from_secs(300))?;
    }

    logging::info("uncordoning node", &[("host", &node.host)]);
    println!("  [{}] Uncordoning...", node.host);
    if let Err(err) = init_k3s.uncordon_node(ctx, &node.host) {
        logging::warn("uncordon failed", &[("host", &node.host), ("error", &err)]);
    }

    let _ = ctx.sleep(settle_delay);
    match init_k3s.get_node_status(ctx) {
        Err(err) => logging::warn(
            "post-upgrade health check failed",
            &[("host", &node.host), ("error", &err)],
        ),
        Ok(out) if out.contains("NotReady") => {
            logging::warn("some nodes NotReady after upgrade", &[("host", &node.host)]);
        }
        Ok(_) => {}
    }

    println!("  [{}] ✅ Upgraded successfully", node.host);
    Ok(())
}

/// Creates an etcd snapshot on the init node and downloads it locally.
pub fn backup(ctx: &Context, cfg: &Config, output_dir: &str) -> Result<(), String> {
    backup_with_runner_factory(ctx, cfg, output_dir, new_runner)
}

/// Creates an etcd snapshot using a provided runner factory.
pub fn backup_with_runner_factory<F>(
    ctx: &Context,
    cfg: &Config,
    output_dir: &str,
    make_runner: F,
) -> Result<(), String>
where
    F: Fn(&NodeConfig) -> Runner,
{
    let init_node = cfg.init_node();
    let init_runner = make_runner(&init_node);
    let init_k3s = k3s::Manager::new(init_runner);

    logging::info("creating etcd snapshot", &[("host", &init_node.host)]);
    println!("📸 Creating etcd snapshot on {}...", init_node.host);

    let snapshot_name = init_k3s
        .create_snapshot(ctx)
        .map_err(|e| format!("[{}] creating snapshot: {}", init_node.host, e))?;

    println!("  Snapshot created: {}", snapshot_name);

    let base_name = Path::new(&snapshot_name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("snapshot");
    let local_path = Path::new(output_dir)
        .join(base_name)
        .to_string_lossy()
        .to_string();

    init_k3s
        .download_snapshot(ctx, &snapshot_name, &local_path)
        .map_err(|e| format!("[{}] downloading snapshot: {}", init_node.host, e))?;

    println!("  📁 Saved to: {}", local_path);
    Ok(())
}

/// Restores etcd from a snapshot file.
pub fn restore(ctx: &Context, cfg: &Config, snapshot_path: &str) -> Result<(), String> {
    restore_with_runner_factory(ctx, cfg, snapshot_path, new_runner)
}

/// Restores etcd from a snapshot file using a provided runner factory.
pub fn restore_with_runner_factory<F>(
    ctx: &Context,
    cfg: &Config,
    snapshot_path: &str,
    make_runner: F,
) -> Result<(), String>
where
    F: Fn(&NodeConfig) -> Runner,
{
    let init_node = cfg.init_node();
    let init_runner = make_runner(&init_node);
    let init_k3s = k3s::Manager::new(init_runner);

    logging::info(
        "restoring from snapshot",
        &[("host", &init_node.host), ("snapshot", snapshot_path)],
    );
    println!(
        "🔄 Restoring etcd from {} on {}...",
        snapshot_path, init_node.host
    );

    let remote_path = "/tmp/homelab-restore-snapshot";
    init_k3s
        .upload_snapshot(ctx, snapshot_path, remote_path)
        .map_err(|e| format!("[{}] uploading snapshot: {}", init_node.host, e))?;

    println!("  Stopping K3s...");
    init_k3s
        .stop_service(ctx)
        .map_err(|e| format!("[{}] stopping K3s: {}", init_node.host, e))?;

    init_k3s
        .restore_snapshot(ctx, remote_path)
        .map_err(|e| format!("[{}] restoring snapshot: {}", init_node.host, e))?;

    println!("  Restarting K3s...");
    init_k3s
        .start_service(ctx)
        .map_err(|e| format!("[{}] starting K3s: {}", init_node.host, e))?;

    init_k3s.wait_for_ready(ctx, Duration::from_secs(300))?;

    println!("  ✅ Restore complete!");
    Ok(())
}

fn get_init_ip<F>(ctx: &Context, cfg: &Config, make_runner: &F) -> String
where
    F: Fn(&NodeConfig) -> Runner,
{
    let init_node = cfg.init_node();
    let runner = make_runner(&init_node);
    let mgr = lima::Manager::new(runner, init_node);
    mgr.get_bridged_ip(ctx).unwrap_or_default()
}

pub fn new_runner(node: &NodeConfig) -> Runner {
    if !node.ssh_user.is_empty() || !node.ssh_port.is_empty() || !node.ssh_key_path.is_empty() {
        Runner::with_opts(
            &node.host,
            &node.ssh_user,
            &node.ssh_port,
            &node.ssh_key_path,
        )
    } else {
        Runner::new(&node.host)
    }
}

fn validate_network<F>(
    ctx: &Context,
    cfg: &Config,
    ip_map: &HashMap<String, String>,
    make_runner: &F,
) -> Result<(), String>
where
    F: Fn(&NodeConfig) -> Runner,
{
    let mut failures = Vec::new();

    for from in &cfg.nodes {
        let runner = make_runner(from);
        for to in &cfg.nodes {
            if from.host == to.host {
                continue;
            }
            let to_ip = ip_map.get(&to.host).map(String::as_str).unwrap_or("");
            if runner
                .lima_shell(
                    ctx,
                    from.get_vm_name(),
                    &format!("ping -c 1 -W 3 {}", to_ip),
                )
                .is_err()
            {
                failures.push(format!("  {} → {} ({}): FAILED", from.host, to.host, to_ip));
                logging::warn(
                    "network check failed",
                    &[("from", &from.host), ("to", &to.host), ("to_ip", to_ip)],
                );
            } else {
                logging::debug(
                    "network check passed",
                    &[("from", &from.host), ("to", &to.host), ("to_ip", to_ip)],
                );
            }
        }
    }

    if !failures.is_empty() {
        return Err(format!(
            "network validation failed — the following paths are broken:\n{}\n\nEnsure all VMs are bridged to the same LAN via socket_vmnet",
            failures.join("\n")
        ));
    }

    println!("  ✅ All VMs can reach each other");
    Ok(())
}

fn export_kubeconfig(
    ctx: &Context,
    k3s_mgr: &k3s::Manager,
    server_ip: &str,
    kubeconfig_path: &str,
) -> Result<(), String> {
    let kubeconfig = k3s_mgr
        .get_kubeconfig(ctx, server_ip)
        .map_err(|e| format!("getting kubeconfig: {}", e))?;

    let mut out_path = expand_path(kubeconfig_path);
    if out_path.is_empty() {
        out_path = expand_path("~/.kube/config");
    }

    if let Some(dir) = Path::new(&out_path).parent() {
        fs::create_dir_all(dir).map_err(|e| format!("creating kubeconfig directory: {}", e))?;
    }

    fs::write(&out_path, kubeconfig.as_bytes())
        .map_err(|e| format!("writing kubeconfig: {}", e))?;

    logging::info("kubeconfig written", &[("path", &out_path)]);
    Ok(())
}

/// Expands a leading `~/` to the current user's `HOME` directory.
pub fn expand_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = env::var("HOME") {
            return Path::new(&home).join(rest).to_string_lossy().to_string();
        }
    }
    path.to_string()
}

/// Returns a new slice of `NodeConfig` in reverse order.
pub fn reverse_nodes(nodes: &[NodeConfig]) -> Vec<NodeConfig> {
    let mut rev = nodes.to_vec();
    rev.reverse();
    rev
}

/// Formats the dry-run plan string for `homelab init --dry-run`.
pub fn format_dry_run_plan(cfg: &Config) -> String {
    let mut out = String::new();
    out.push_str("\n📋 Dry-run plan:\n\n");
    let init_host = cfg.init_node().host;
    for node in cfg.server_nodes() {
        let role = if node.host == init_host {
            "server (init)"
        } else {
            "server"
        };
        out.push_str(&format!(
            "  [{}] Provision VM → Install K3s {} → Label pool={}\n",
            node.host, role, node.pool
        ));
    }
    for node in cfg.agent_nodes() {
        out.push_str(&format!(
            "  [{}] Provision VM → Install K3s agent → Label pool={}\n",
            node.host, node.pool
        ));
    }
    out.push_str("\n  No changes were made (dry-run mode).");
    out
}

/// Prints the dry-run plan to stdout.
pub fn print_dry_run_plan(cfg: &Config) {
    println!("{}", format_dry_run_plan(cfg));
}
