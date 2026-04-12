// Package k3s handles K3s installation, cluster join, and lifecycle management
// on remote Lima VMs.
package k3s

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/VitruvianSoftware/homelab/internal/remote"
)

const vmName = "k8s-node"

// Manager handles K3s operations on a remote Lima VM.
type Manager struct {
	runner *remote.Runner
}

// NewManager creates a new K3s manager for the given remote host.
func NewManager(runner *remote.Runner) *Manager {
	return &Manager{runner: runner}
}

// IsInstalled checks whether K3s is installed inside the VM.
func (m *Manager) IsInstalled(ctx context.Context) (bool, error) {
	out, err := m.runner.LimaShellSudo(ctx, vmName, "test -f /usr/local/bin/k3s && echo yes || echo no")
	if err != nil {
		return false, err
	}
	return strings.TrimSpace(out) == "yes", nil
}

// InitCluster bootstraps the first control plane node with --cluster-init.
func (m *Manager) InitCluster(ctx context.Context, nodeIP, pool, k3sVersion string, tlsSANs []string) error {
	installed, err := m.IsInstalled(ctx)
	if err != nil {
		return err
	}
	if installed {
		fmt.Printf("  [%s] K3s already installed, skipping init\n", m.runner.Host)
		return nil
	}

	fmt.Printf("  [%s] Initializing K3s cluster...\n", m.runner.Host)

	var sanFlags string
	for _, san := range tlsSANs {
		sanFlags += fmt.Sprintf(" --tls-san=%s", san)
	}

	versionEnv := ""
	if k3sVersion != "" {
		versionEnv = fmt.Sprintf("INSTALL_K3S_VERSION=%q ", k3sVersion)
	}

	script := fmt.Sprintf(
		`curl -sfL https://get.k3s.io | %sINSTALL_K3S_EXEC="server" sh -s - --cluster-init --node-ip=%s --advertise-address=%s%s --flannel-iface=lima0 --node-label=pool=%s`,
		versionEnv, nodeIP, nodeIP, sanFlags, pool,
	)

	_, err = m.runner.LimaShellSudo(ctx, vmName, script)
	return err
}

// JoinServer joins a server node to an existing HA cluster.
func (m *Manager) JoinServer(ctx context.Context, nodeIP, serverURL, token, pool, k3sVersion string, tlsSANs []string) error {
	installed, err := m.IsInstalled(ctx)
	if err != nil {
		return err
	}
	if installed {
		fmt.Printf("  [%s] K3s already installed, skipping join\n", m.runner.Host)
		return nil
	}

	fmt.Printf("  [%s] Joining cluster as server...\n", m.runner.Host)

	var sanFlags string
	for _, san := range tlsSANs {
		sanFlags += fmt.Sprintf(" --tls-san=%s", san)
	}

	versionEnv := ""
	if k3sVersion != "" {
		versionEnv = fmt.Sprintf("INSTALL_K3S_VERSION=%q ", k3sVersion)
	}

	script := fmt.Sprintf(
		`curl -sfL https://get.k3s.io | %sK3S_TOKEN=%q INSTALL_K3S_EXEC="server" sh -s - --server=%s --node-ip=%s --advertise-address=%s%s --flannel-iface=lima0 --node-label=pool=%s`,
		versionEnv, token, serverURL, nodeIP, nodeIP, sanFlags, pool,
	)

	_, err = m.runner.LimaShellSudo(ctx, vmName, script)
	return err
}

// JoinAgent joins a worker node to the cluster.
func (m *Manager) JoinAgent(ctx context.Context, nodeIP, serverURL, token, pool, k3sVersion string) error {
	installed, err := m.IsInstalled(ctx)
	if err != nil {
		return err
	}
	if installed {
		fmt.Printf("  [%s] K3s already installed, skipping join\n", m.runner.Host)
		return nil
	}

	fmt.Printf("  [%s] Joining cluster as agent...\n", m.runner.Host)

	versionEnv := ""
	if k3sVersion != "" {
		versionEnv = fmt.Sprintf("INSTALL_K3S_VERSION=%q ", k3sVersion)
	}

	script := fmt.Sprintf(
		`curl -sfL https://get.k3s.io | %sK3S_TOKEN=%q K3S_URL=%q sh -s - agent --node-ip=%s --flannel-iface=lima0 --node-label=pool=%s`,
		versionEnv, token, serverURL, nodeIP, pool,
	)

	_, err = m.runner.LimaShellSudo(ctx, vmName, script)
	return err
}

// GetToken retrieves the K3s node token from the server.
func (m *Manager) GetToken(ctx context.Context) (string, error) {
	out, err := m.runner.LimaShellSudo(ctx, vmName, "cat /var/lib/rancher/k3s/server/node-token")
	if err != nil {
		return "", fmt.Errorf("reading node token: %w", err)
	}
	return strings.TrimSpace(out), nil
}

// WaitForReady polls until K3s is initialized and the node token exists.
func (m *Manager) WaitForReady(ctx context.Context, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)

	for time.Now().Before(deadline) {
		out, err := m.runner.LimaShellSudo(ctx, vmName, "test -f /var/lib/rancher/k3s/server/node-token && echo ready || echo waiting")
		if err == nil && strings.TrimSpace(out) == "ready" {
			return nil
		}
		fmt.Printf("  [%s] Waiting for K3s to initialize...\n", m.runner.Host)
		time.Sleep(10 * time.Second)
	}

	return fmt.Errorf("timed out waiting for K3s to become ready on %s", m.runner.Host)
}

// GetNodeStatus returns the output of kubectl get nodes.
func (m *Manager) GetNodeStatus(ctx context.Context) (string, error) {
	return m.runner.LimaShellSudo(ctx, vmName, "k3s kubectl get nodes -o wide -L pool -L kubernetes.io/arch")
}

// Uninstall removes K3s from the VM.
func (m *Manager) Uninstall(ctx context.Context, role string) error {
	installed, err := m.IsInstalled(ctx)
	if err != nil {
		return err
	}
	if !installed {
		return nil
	}

	fmt.Printf("  [%s] Uninstalling K3s...\n", m.runner.Host)

	script := "/usr/local/bin/k3s-uninstall.sh"
	if role == "agent" {
		script = "/usr/local/bin/k3s-agent-uninstall.sh"
	}

	_, err = m.runner.LimaShellSudo(ctx, vmName, script)
	return err
}

// DrainNode drains a node before removal.
func (m *Manager) DrainNode(ctx context.Context, nodeName string) error {
	fmt.Printf("  [%s] Draining node %s...\n", m.runner.Host, nodeName)
	_, err := m.runner.LimaShellSudo(ctx, vmName,
		fmt.Sprintf("k3s kubectl drain %s --ignore-daemonsets --delete-emptydir-data --timeout=60s", nodeName))
	return err
}

// DeleteNode removes a node from the cluster.
func (m *Manager) DeleteNode(ctx context.Context, nodeName string) error {
	fmt.Printf("  [%s] Deleting node %s from cluster...\n", m.runner.Host, nodeName)
	_, err := m.runner.LimaShellSudo(ctx, vmName,
		fmt.Sprintf("k3s kubectl delete node %s", nodeName))
	return err
}

// GetKubeconfig retrieves and patches the kubeconfig for external access.
func (m *Manager) GetKubeconfig(ctx context.Context, serverIP string) (string, error) {
	out, err := m.runner.LimaShellSudo(ctx, vmName, "cat /etc/rancher/k3s/k3s.yaml")
	if err != nil {
		return "", err
	}
	// Replace localhost with the actual server IP.
	return strings.ReplaceAll(out, "127.0.0.1", serverIP), nil
}
