// Package cluster orchestrates the full lifecycle of a homelab K8s cluster.
// It coordinates Lima VM provisioning, K3s installation, and cluster operations
// across multiple remote macOS hosts.
package cluster

import (
	"context"
	"fmt"
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
func Init(ctx context.Context, cfg *config.Config) error {
	fmt.Println("🚀 Initializing homelab cluster:", cfg.Cluster.Name)

	// Phase 1: Provision all Lima VMs in sequence.
	fmt.Println("\n📦 Phase 1: Provisioning VMs...")
	ipMap := make(map[string]string) // host -> bridged IP

	for _, node := range cfg.Nodes {
		runner := remote.NewRunner(node.Host)
		mgr := lima.NewManager(runner, node)

		if err := mgr.Provision(ctx); err != nil {
			return fmt.Errorf("provisioning VM on %s: %w", node.Host, err)
		}

		ip, err := mgr.GetBridgedIP(ctx)
		if err != nil {
			return fmt.Errorf("getting bridged IP for %s: %w", node.Host, err)
		}
		ipMap[node.Host] = ip
		fmt.Printf("  [%s] Bridged IP: %s\n", node.Host, ip)
	}

	// Collect all server IPs for TLS SANs.
	var serverIPs []string
	for _, node := range cfg.ServerNodes() {
		serverIPs = append(serverIPs, ipMap[node.Host])
	}

	// Phase 2: Initialize the first control plane node.
	fmt.Println("\n🔧 Phase 2: Bootstrapping control plane (CP-1)...")
	initNode := cfg.InitNode()
	initRunner := remote.NewRunner(initNode.Host)
	initK3s := k3s.NewManager(initRunner)

	if err := initK3s.InitCluster(ctx, ipMap[initNode.Host], initNode.Pool, cfg.Cluster.K3sVersion, serverIPs); err != nil {
		return fmt.Errorf("initializing K3s on %s: %w", initNode.Host, err)
	}

	if err := initK3s.WaitForReady(ctx, 5*time.Minute); err != nil {
		return err
	}

	token, err := initK3s.GetToken(ctx)
	if err != nil {
		return err
	}
	fmt.Printf("  [%s] Cluster initialized, token acquired\n", initNode.Host)

	serverURL := fmt.Sprintf("https://%s:6443", ipMap[initNode.Host])

	// Phase 3: Join remaining server nodes.
	servers := cfg.ServerNodes()
	if len(servers) > 1 {
		fmt.Println("\n🔗 Phase 3: Joining control plane nodes...")
		for _, node := range servers[1:] {
			runner := remote.NewRunner(node.Host)
			k3sMgr := k3s.NewManager(runner)

			if err := k3sMgr.JoinServer(ctx, ipMap[node.Host], serverURL, token, node.Pool, cfg.Cluster.K3sVersion, []string{ipMap[node.Host]}); err != nil {
				return fmt.Errorf("joining server %s: %w", node.Host, err)
			}

			if err := k3sMgr.WaitForReady(ctx, 5*time.Minute); err != nil {
				return err
			}
			fmt.Printf("  [%s] Joined as server\n", node.Host)
		}
	}

	// Phase 4: Join agent nodes.
	agents := cfg.AgentNodes()
	if len(agents) > 0 {
		fmt.Println("\n🔗 Phase 4: Joining worker nodes...")
		for _, node := range agents {
			runner := remote.NewRunner(node.Host)
			k3sMgr := k3s.NewManager(runner)

			if err := k3sMgr.JoinAgent(ctx, ipMap[node.Host], serverURL, token, node.Pool, cfg.Cluster.K3sVersion); err != nil {
				return fmt.Errorf("joining agent %s: %w", node.Host, err)
			}
			fmt.Printf("  [%s] Joined as agent\n", node.Host)
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
		return fmt.Errorf("getting node status: %w", err)
	}
	fmt.Println(out)
	fmt.Printf("\n🎉 Cluster %q is ready!\n", cfg.Cluster.Name)

	return nil
}

// Join adds nodes that are in the config but not yet in the cluster.
func Join(ctx context.Context, cfg *config.Config) error {
	fmt.Println("🔗 Joining new nodes to cluster:", cfg.Cluster.Name)

	// Get the init node to query current cluster state and retrieve the token.
	initNode := cfg.InitNode()
	initRunner := remote.NewRunner(initNode.Host)
	initK3s := k3s.NewManager(initRunner)
	initLima := lima.NewManager(initRunner, initNode)

	initIP, err := initLima.GetBridgedIP(ctx)
	if err != nil {
		return fmt.Errorf("getting init node IP: %w", err)
	}

	token, err := initK3s.GetToken(ctx)
	if err != nil {
		return fmt.Errorf("getting cluster token: %w", err)
	}

	serverURL := fmt.Sprintf("https://%s:6443", initIP)

	for _, node := range cfg.Nodes {
		runner := remote.NewRunner(node.Host)
		k3sMgr := k3s.NewManager(runner)
		limaMgr := lima.NewManager(runner, node)

		installed, _ := k3sMgr.IsInstalled(ctx)
		if installed {
			fmt.Printf("  [%s] Already part of cluster, skipping\n", node.Host)
			continue
		}

		// Ensure VM is provisioned.
		if err := limaMgr.Provision(ctx); err != nil {
			return fmt.Errorf("provisioning %s: %w", node.Host, err)
		}

		nodeIP, err := limaMgr.GetBridgedIP(ctx)
		if err != nil {
			return fmt.Errorf("getting IP for %s: %w", node.Host, err)
		}

		switch node.Role {
		case "server":
			if err := k3sMgr.JoinServer(ctx, nodeIP, serverURL, token, node.Pool, cfg.Cluster.K3sVersion, []string{nodeIP}); err != nil {
				return fmt.Errorf("joining server %s: %w", node.Host, err)
			}
		case "agent":
			if err := k3sMgr.JoinAgent(ctx, nodeIP, serverURL, token, node.Pool, cfg.Cluster.K3sVersion); err != nil {
				return fmt.Errorf("joining agent %s: %w", node.Host, err)
			}
		}

		fmt.Printf("  [%s] Joined as %s\n", node.Host, node.Role)
	}

	return nil
}

// Remove drains and removes a specific node from the cluster.
func Remove(ctx context.Context, cfg *config.Config, hostName string) error {
	fmt.Println("🗑️  Removing node:", hostName)

	// Find the node in the config.
	var target *config.NodeConfig
	for _, n := range cfg.Nodes {
		if n.Host == hostName {
			n := n
			target = &n
			break
		}
	}
	if target == nil {
		return fmt.Errorf("node %q not found in config", hostName)
	}

	// Use the init node to drain and delete from the cluster.
	initNode := cfg.InitNode()
	initRunner := remote.NewRunner(initNode.Host)
	initK3s := k3s.NewManager(initRunner)

	// Drain the node.
	if err := initK3s.DrainNode(ctx, hostName); err != nil {
		fmt.Printf("  Warning: drain failed (may already be removed): %v\n", err)
	}

	// Delete the node from K8s.
	if err := initK3s.DeleteNode(ctx, hostName); err != nil {
		fmt.Printf("  Warning: delete failed (may already be removed): %v\n", err)
	}

	// Uninstall K3s on the target.
	targetRunner := remote.NewRunner(target.Host)
	targetK3s := k3s.NewManager(targetRunner)
	if err := targetK3s.Uninstall(ctx, target.Role); err != nil {
		return fmt.Errorf("uninstalling K3s on %s: %w", target.Host, err)
	}

	fmt.Printf("  [%s] Node removed successfully\n", hostName)
	return nil
}

// Destroy tears down the entire cluster.
func Destroy(ctx context.Context, cfg *config.Config, force bool) error {
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
		runner := remote.NewRunner(node.Host)
		k3sMgr := k3s.NewManager(runner)
		limaMgr := lima.NewManager(runner, node)

		// Uninstall K3s.
		if err := k3sMgr.Uninstall(ctx, node.Role); err != nil {
			fmt.Printf("  Warning: K3s uninstall on %s failed: %v\n", node.Host, err)
		}

		// Destroy the Lima VM.
		if err := limaMgr.Destroy(ctx); err != nil {
			fmt.Printf("  Warning: VM destroy on %s failed: %v\n", node.Host, err)
		}
	}

	// Remove kubeconfig.
	kubeconfigPath := expandPath(cfg.Cluster.Kubeconfig)
	if kubeconfigPath != "" {
		os.Remove(kubeconfigPath)
		fmt.Printf("  Removed kubeconfig: %s\n", kubeconfigPath)
	}

	fmt.Printf("\n🗑️  Cluster %q destroyed.\n", cfg.Cluster.Name)
	return nil
}

// Status displays the current state of all configured nodes.
func Status(ctx context.Context, cfg *config.Config) error {
	fmt.Println("📊 Cluster status:", cfg.Cluster.Name)
	fmt.Println()

	for _, node := range cfg.Nodes {
		runner := remote.NewRunner(node.Host)
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
	initRunner := remote.NewRunner(initNode.Host)
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

	fmt.Printf("  Kubeconfig written to %s\n", outPath)
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
