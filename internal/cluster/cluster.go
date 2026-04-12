// Package cluster orchestrates the full lifecycle of a homelab K8s cluster.
// It coordinates Lima VM provisioning, K3s installation, and cluster operations
// across multiple remote macOS hosts.
package cluster

import (
	"context"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/VitruvianSoftware/homelab/internal/config"
	"github.com/VitruvianSoftware/homelab/internal/k3s"
	"github.com/VitruvianSoftware/homelab/internal/lima"
	"github.com/VitruvianSoftware/homelab/internal/remote"
)

// Init bootstraps a new cluster from scratch. It is idempotent.
func Init(ctx context.Context, cfg *config.Config, dryRun bool) error {
	slog.Info("initializing homelab cluster", "name", cfg.Cluster.Name, "nodes", len(cfg.Nodes), "dry_run", dryRun)

	// Phase 1: Provision all Lima VMs.
	fmt.Println("📦 Phase 1: Provisioning VMs...")
	ipMap := make(map[string]string) // host -> bridged IP

	for _, node := range cfg.Nodes {
		runner := newRunner(node)
		mgr := lima.NewManager(runner, node)

		if dryRun {
			fmt.Printf("  [%s] Would provision VM (cpus=%d, memory=%s, disk=%s)\n",
				node.Host, node.VM.CPUs, node.VM.Memory, node.VM.Disk)
			ipMap[node.Host] = "<pending>"
			continue
		}

		if err := mgr.Provision(ctx); err != nil {
			return fmt.Errorf("[%s] provisioning VM: %w", node.Host, err)
		}

		ip, err := mgr.GetBridgedIP(ctx)
		if err != nil {
			return fmt.Errorf("[%s] getting bridged IP: %w", node.Host, err)
		}
		ipMap[node.Host] = ip
		slog.Info("VM provisioned", "host", node.Host, "ip", ip)
	}

	if dryRun {
		printDryRunPlan(cfg)
		return nil
	}

	// Collect all server IPs for TLS SANs.
	var serverIPs []string
	for _, node := range cfg.ServerNodes() {
		serverIPs = append(serverIPs, ipMap[node.Host])
	}

	// Phase 2: Initialize the first control plane node.
	fmt.Println("\n🔧 Phase 2: Bootstrapping control plane (CP-1)...")
	initNode := cfg.InitNode()
	initRunner := newRunner(initNode)
	initK3s := k3s.NewManager(initRunner)

	if err := initK3s.InitCluster(ctx, ipMap[initNode.Host], initNode.Pool, cfg.Cluster.K3sVersion, serverIPs); err != nil {
		return fmt.Errorf("[%s] initializing K3s: %w\n\nRecovery: this command is idempotent, re-run 'homelab init' to retry", initNode.Host, err)
	}

	if err := initK3s.WaitForReady(ctx, 5*time.Minute); err != nil {
		return err
	}

	token, err := initK3s.GetToken(ctx)
	if err != nil {
		return fmt.Errorf("[%s] getting token: %w", initNode.Host, err)
	}
	slog.Info("cluster initialized, token acquired", "host", initNode.Host)

	serverURL := fmt.Sprintf("https://%s:6443", ipMap[initNode.Host])

	// Phase 3: Join remaining server nodes.
	servers := cfg.ServerNodes()
	if len(servers) > 1 {
		fmt.Println("\n🔗 Phase 3: Joining control plane nodes...")
		for _, node := range servers[1:] {
			runner := newRunner(node)
			k3sMgr := k3s.NewManager(runner)

			if err := k3sMgr.JoinServer(ctx, ipMap[node.Host], serverURL, token, node.Pool, cfg.Cluster.K3sVersion, []string{ipMap[node.Host]}); err != nil {
				return fmt.Errorf("[%s] joining as server: %w\n\nRecovery: re-run 'homelab init' to retry", node.Host, err)
			}

			if err := k3sMgr.WaitForReady(ctx, 5*time.Minute); err != nil {
				return err
			}
			slog.Info("node joined as server", "host", node.Host)
		}
	}

	// Phase 4: Join agent nodes.
	agents := cfg.AgentNodes()
	if len(agents) > 0 {
		fmt.Println("\n🔗 Phase 4: Joining worker nodes...")
		for _, node := range agents {
			runner := newRunner(node)
			k3sMgr := k3s.NewManager(runner)

			if err := k3sMgr.JoinAgent(ctx, ipMap[node.Host], serverURL, token, node.Pool, cfg.Cluster.K3sVersion); err != nil {
				return fmt.Errorf("[%s] joining as agent: %w\n\nRecovery: re-run 'homelab init' to retry", node.Host, err)
			}
			slog.Info("node joined as agent", "host", node.Host)
		}
	}

	// Phase 5: Export kubeconfig.
	fmt.Println("\n📋 Phase 5: Exporting kubeconfig...")
	if err := exportKubeconfig(ctx, initK3s, ipMap[initNode.Host], cfg.Cluster.Kubeconfig); err != nil {
		return err
	}

	// Phase 6: Final validation.
	fmt.Println("\n✅ Phase 6: Validating cluster...")
	out, err := initK3s.GetNodeStatus(ctx)
	if err != nil {
		return fmt.Errorf("[%s] getting node status: %w", initNode.Host, err)
	}
	fmt.Println(out)
	fmt.Printf("\n🎉 Cluster %q is ready!\n", cfg.Cluster.Name)

	return nil
}

// Join adds nodes that are in the config but not yet in the cluster.
func Join(ctx context.Context, cfg *config.Config, dryRun bool) error {
	slog.Info("joining new nodes to cluster", "name", cfg.Cluster.Name, "dry_run", dryRun)

	// Get the init node to query current cluster state and retrieve the token.
	initNode := cfg.InitNode()
	initRunner := newRunner(initNode)
	initK3s := k3s.NewManager(initRunner)
	initLima := lima.NewManager(initRunner, initNode)

	initIP, err := initLima.GetBridgedIP(ctx)
	if err != nil {
		return fmt.Errorf("[%s] getting init node IP: %w", initNode.Host, err)
	}

	token, err := initK3s.GetToken(ctx)
	if err != nil {
		return fmt.Errorf("[%s] getting cluster token: %w", initNode.Host, err)
	}

	serverURL := fmt.Sprintf("https://%s:6443", initIP)

	for _, node := range cfg.Nodes {
		runner := newRunner(node)
		k3sMgr := k3s.NewManager(runner)
		limaMgr := lima.NewManager(runner, node)

		installed, _ := k3sMgr.IsInstalled(ctx)
		if installed {
			slog.Debug("node already part of cluster, skipping", "host", node.Host)
			continue
		}

		if dryRun {
			fmt.Printf("  [%s] Would join as %s (pool=%s)\n", node.Host, node.Role, node.Pool)
			continue
		}

		// Ensure VM is provisioned.
		if err := limaMgr.Provision(ctx); err != nil {
			return fmt.Errorf("[%s] provisioning: %w", node.Host, err)
		}

		nodeIP, err := limaMgr.GetBridgedIP(ctx)
		if err != nil {
			return fmt.Errorf("[%s] getting IP: %w", node.Host, err)
		}

		switch node.Role {
		case "server":
			if err := k3sMgr.JoinServer(ctx, nodeIP, serverURL, token, node.Pool, cfg.Cluster.K3sVersion, []string{nodeIP}); err != nil {
				return fmt.Errorf("[%s] joining as server: %w", node.Host, err)
			}
		case "agent":
			if err := k3sMgr.JoinAgent(ctx, nodeIP, serverURL, token, node.Pool, cfg.Cluster.K3sVersion); err != nil {
				return fmt.Errorf("[%s] joining as agent: %w", node.Host, err)
			}
		}

		slog.Info("node joined", "host", node.Host, "role", node.Role)
	}

	return nil
}

// Remove drains and removes a specific node from the cluster.
func Remove(ctx context.Context, cfg *config.Config, hostName string, dryRun bool) error {
	slog.Info("removing node", "host", hostName, "dry_run", dryRun)

	// Find the node in the config.
	var target *config.NodeConfig
	for _, n := range cfg.Nodes {
		n := n
		if n.Host == hostName {
			target = &n
			break
		}
	}
	if target == nil {
		return fmt.Errorf("node %q not found in config", hostName)
	}

	if dryRun {
		fmt.Printf("  Would drain and remove %s (%s) from cluster\n", hostName, target.Role)
		return nil
	}

	// Use the init node to drain and delete from the cluster.
	initNode := cfg.InitNode()
	initRunner := newRunner(initNode)
	initK3s := k3s.NewManager(initRunner)

	// Drain the node.
	if err := initK3s.DrainNode(ctx, hostName); err != nil {
		slog.Warn("drain failed (may already be removed)", "host", hostName, "error", err)
	}

	// Delete the node from K8s.
	if err := initK3s.DeleteNode(ctx, hostName); err != nil {
		slog.Warn("delete failed (may already be removed)", "host", hostName, "error", err)
	}

	// Uninstall K3s on the target.
	targetRunner := newRunner(*target)
	targetK3s := k3s.NewManager(targetRunner)
	if err := targetK3s.Uninstall(ctx, target.Role); err != nil {
		return fmt.Errorf("[%s] uninstalling K3s: %w", target.Host, err)
	}

	slog.Info("node removed successfully", "host", hostName)
	return nil
}

// Destroy tears down the entire cluster.
func Destroy(ctx context.Context, cfg *config.Config, force, dryRun bool) error {
	slog.Info("destroying cluster", "name", cfg.Cluster.Name, "force", force, "dry_run", dryRun)

	if dryRun {
		fmt.Printf("Would destroy cluster %q:\n", cfg.Cluster.Name)
		for _, node := range cfg.Nodes {
			fmt.Printf("  [%s] Would uninstall K3s (%s) and destroy VM\n", node.Host, node.Role)
		}
		return nil
	}

	if !force {
		fmt.Printf("⚠️  This will destroy the entire cluster %q. Are you sure? [y/N] ", cfg.Cluster.Name)
		var confirm string
		fmt.Scanln(&confirm)
		if strings.ToLower(confirm) != "y" {
			fmt.Println("Aborted.")
			return nil
		}
	}

	fmt.Println("💥 Destroying cluster:", cfg.Cluster.Name)

	// Uninstall K3s and destroy VMs on all nodes (agents first, then servers in reverse).
	allNodes := append(cfg.AgentNodes(), reverseNodes(cfg.ServerNodes())...)

	for _, node := range allNodes {
		runner := newRunner(node)
		k3sMgr := k3s.NewManager(runner)
		limaMgr := lima.NewManager(runner, node)

		// Uninstall K3s.
		if err := k3sMgr.Uninstall(ctx, node.Role); err != nil {
			slog.Warn("K3s uninstall failed", "host", node.Host, "error", err)
		}

		// Destroy the Lima VM.
		if err := limaMgr.Destroy(ctx); err != nil {
			slog.Warn("VM destroy failed", "host", node.Host, "error", err)
		}
	}

	// Remove kubeconfig.
	kubeconfigPath := expandPath(cfg.Cluster.Kubeconfig)
	if kubeconfigPath != "" {
		os.Remove(kubeconfigPath)
		slog.Info("removed kubeconfig", "path", kubeconfigPath)
	}

	fmt.Printf("\n🗑️  Cluster %q destroyed.\n", cfg.Cluster.Name)
	return nil
}

// Status displays the current state of all configured nodes.
func Status(ctx context.Context, cfg *config.Config) error {
	slog.Info("checking cluster status", "name", cfg.Cluster.Name)

	fmt.Println("📊 Cluster status:", cfg.Cluster.Name)
	fmt.Println()

	for _, node := range cfg.Nodes {
		runner := newRunner(node)
		limaMgr := lima.NewManager(runner, node)
		k3sMgr := k3s.NewManager(runner)

		vmStatus, _ := limaMgr.Status(ctx)
		k3sInstalled, _ := k3sMgr.IsInstalled(ctx)

		k3sStatus := "not installed"
		if k3sInstalled {
			k3sStatus = "installed"
		}

		fmt.Printf("  %-25s role=%-6s pool=%-15s vm=%-12s k3s=%s\n",
			node.Host, node.Role, node.Pool, string(vmStatus), k3sStatus)
	}

	// If any server node is reachable, show kubectl output.
	fmt.Println()
	initNode := cfg.InitNode()
	initRunner := newRunner(initNode)
	initK3s := k3s.NewManager(initRunner)
	out, err := initK3s.GetNodeStatus(ctx)
	if err == nil {
		fmt.Println(out)
	} else {
		fmt.Println("  (Cluster not reachable for node status)")
	}

	return nil
}

// --- helpers ---

// newRunner creates a remote.Runner from a NodeConfig, using optional SSH settings.
func newRunner(node config.NodeConfig) *remote.Runner {
	if node.SSHUser != "" || node.SSHPort != "" || node.SSHKeyPath != "" {
		return remote.NewRunnerWithOpts(node.Host, node.SSHUser, node.SSHPort, node.SSHKeyPath)
	}
	return remote.NewRunner(node.Host)
}

func exportKubeconfig(ctx context.Context, k3sMgr *k3s.Manager, serverIP, kubeconfigPath string) error {
	kubeconfig, err := k3sMgr.GetKubeconfig(ctx, serverIP)
	if err != nil {
		return fmt.Errorf("getting kubeconfig: %w", err)
	}

	outPath := expandPath(kubeconfigPath)
	if outPath == "" {
		outPath = expandPath("~/.kube/config")
	}

	dir := filepath.Dir(outPath)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return fmt.Errorf("creating kubeconfig directory: %w", err)
	}

	if err := os.WriteFile(outPath, []byte(kubeconfig), 0o600); err != nil {
		return fmt.Errorf("writing kubeconfig: %w", err)
	}

	slog.Info("kubeconfig written", "path", outPath)
	return nil
}

func expandPath(path string) string {
	if strings.HasPrefix(path, "~/") {
		home, _ := os.UserHomeDir()
		return filepath.Join(home, path[2:])
	}
	return path
}

func reverseNodes(nodes []config.NodeConfig) []config.NodeConfig {
	reversed := make([]config.NodeConfig, len(nodes))
	for i, n := range nodes {
		reversed[len(nodes)-1-i] = n
	}
	return reversed
}

func printDryRunPlan(cfg *config.Config) {
	fmt.Println("\n📋 Dry-run plan:")
	fmt.Println()
	for _, node := range cfg.ServerNodes() {
		role := "server"
		if node.Host == cfg.InitNode().Host {
			role = "server (init)"
		}
		fmt.Printf("  [%s] Provision VM → Install K3s %s → Label pool=%s\n", node.Host, role, node.Pool)
	}
	for _, node := range cfg.AgentNodes() {
		fmt.Printf("  [%s] Provision VM → Install K3s agent → Label pool=%s\n", node.Host, node.Pool)
	}
	fmt.Println("\n  No changes were made (dry-run mode).")
}
