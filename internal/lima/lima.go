// Package lima handles generating Lima configuration files and managing
// VM lifecycle (create, start, stop, delete, status) on remote hosts.
package lima

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/VitruvianSoftware/homelab/internal/config"
	"github.com/VitruvianSoftware/homelab/internal/remote"
)

const vmName = "k8s-node"

// VMStatus represents the state of a Lima VM.
type VMStatus string

const (
	VMStatusRunning    VMStatus = "Running"
	VMStatusStopped    VMStatus = "Stopped"
	VMStatusNotCreated VMStatus = "NotCreated"
	VMStatusUnknown    VMStatus = "Unknown"
)

// Manager handles Lima VM operations on a remote host.
type Manager struct {
	runner *remote.Runner
	node   config.NodeConfig
}

// NewManager creates a new Lima manager for the given host and node config.
func NewManager(runner *remote.Runner, node config.NodeConfig) *Manager {
	return &Manager{runner: runner, node: node}
}

// Status returns the current VM status on the remote host.
func (m *Manager) Status(ctx context.Context) (VMStatus, error) {
	out, err := m.runner.Run(ctx, "limactl list --json")
	if err != nil {
		if strings.Contains(err.Error(), "command not found") {
			return VMStatusNotCreated, fmt.Errorf("lima is not installed on %s", m.runner.Host)
		}
		return VMStatusUnknown, err
	}

	// limactl list --json outputs one JSON object per line.
	for _, line := range strings.Split(out, "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		var entry struct {
			Name   string `json:"name"`
			Status string `json:"status"`
		}
		if err := json.Unmarshal([]byte(line), &entry); err != nil {
			continue
		}
		if entry.Name == vmName {
			return VMStatus(entry.Status), nil
		}
	}

	return VMStatusNotCreated, nil
}

// GenerateConfig returns the Lima YAML config for this node.
func (m *Manager) GenerateConfig() string {
	return fmt.Sprintf(`vmType: "vz"
rosetta:
  enabled: true
  binfmt: true
images:
  - location: "https://cloud-images.ubuntu.com/releases/24.04/release/ubuntu-24.04-server-cloudimg-arm64.img"
    arch: "aarch64"
  - location: "https://cloud-images.ubuntu.com/releases/24.04/release/ubuntu-24.04-server-cloudimg-amd64.img"
    arch: "x86_64"
cpus: %d
memory: "%s"
disk: "%s"
networks:
  - socket: "/var/run/socket_vmnet"
provision:
  - mode: system
    script: |
      #!/bin/bash
      apt-get update -qq && apt-get install -y -qq curl open-iscsi nfs-common
`, m.node.VM.CPUs, m.node.VM.Memory, m.node.VM.Disk)
}

// Provision creates and starts the Lima VM on the remote host.
func (m *Manager) Provision(ctx context.Context) error {
	status, err := m.Status(ctx)
	if err != nil && status != VMStatusNotCreated {
		return err
	}

	switch status {
	case VMStatusRunning:
		fmt.Printf("  [%s] VM already running, skipping provision\n", m.runner.Host)
		return nil
	case VMStatusStopped:
		fmt.Printf("  [%s] VM exists but stopped, starting...\n", m.runner.Host)
		_, err := m.runner.Run(ctx, fmt.Sprintf("limactl start %s", vmName))
		return err
	case VMStatusNotCreated:
		fmt.Printf("  [%s] Creating and starting VM...\n", m.runner.Host)

		// Write the config file to the remote host.
		limaConfig := m.GenerateConfig()
		_, err := m.runner.RunShell(ctx, fmt.Sprintf("cat > ~/k8s-node.yaml << 'LIMAEOF'\n%s\nLIMAEOF", limaConfig))
		if err != nil {
			return fmt.Errorf("writing lima config: %w", err)
		}

		// Start the VM.
		_, err = m.runner.Run(ctx, fmt.Sprintf("limactl start --name=%s ~/k8s-node.yaml --tty=false", vmName))
		if err != nil {
			return fmt.Errorf("starting lima VM: %w", err)
		}

		return nil
	default:
		return fmt.Errorf("unexpected VM status: %s", status)
	}
}

// GetBridgedIP returns the bridged LAN IP address of the VM.
func (m *Manager) GetBridgedIP(ctx context.Context) (string, error) {
	// Try lima0 interface first (socket_vmnet typically creates this).
	out, err := m.runner.LimaShell(ctx, vmName, "ip -4 addr show lima0 2>/dev/null | grep -oP 'inet \\K[0-9.]+'")
	if err == nil && out != "" {
		return strings.TrimSpace(out), nil
	}

	// Fallback: use ip route to find the source IP for external traffic.
	out, err = m.runner.LimaShell(ctx, vmName, "ip -4 route get 1.1.1.1 | grep -oP 'src \\K[0-9.]+'")
	if err != nil {
		return "", fmt.Errorf("could not determine bridged IP: %w", err)
	}

	return strings.TrimSpace(out), nil
}

// Destroy stops and deletes the Lima VM.
func (m *Manager) Destroy(ctx context.Context) error {
	status, err := m.Status(ctx)
	if err != nil && status != VMStatusNotCreated {
		return err
	}

	if status == VMStatusNotCreated {
		fmt.Printf("  [%s] No VM found, skipping\n", m.runner.Host)
		return nil
	}

	fmt.Printf("  [%s] Destroying VM...\n", m.runner.Host)
	_, err = m.runner.Run(ctx, fmt.Sprintf("limactl stop %s --force 2>/dev/null; limactl delete %s --force", vmName, vmName))
	return err
}
