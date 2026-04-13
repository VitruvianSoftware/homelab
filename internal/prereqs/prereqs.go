// Package prereqs checks and optionally installs prerequisites on remote macOS hosts.
package prereqs

import (
	"context"
	"fmt"
	"log/slog"
	"strings"

	"github.com/VitruvianSoftware/homelab/internal/config"
	"github.com/VitruvianSoftware/homelab/internal/remote"
)

// CheckResult holds the result of a single prerequisite check.
type CheckResult struct {
	Name      string
	Installed bool
	Message   string
}

// EnsureAll verifies and optionally installs prerequisites on a single host.
// If autoInstall is true, missing tools will be installed automatically.
func EnsureAll(ctx context.Context, node config.NodeConfig, autoInstall bool) error {
	runner := newRunner(node)
	host := node.Host

	slog.Info("checking prerequisites", "host", host, "auto_install", autoInstall)
	fmt.Printf("  [%s] Checking prerequisites...\n", host)

	// 1. Check Homebrew.
	if err := checkOrInstallBrew(ctx, runner, host, autoInstall); err != nil {
		return err
	}

	// 2. Check Lima.
	if err := checkOrInstallLima(ctx, runner, host, autoInstall); err != nil {
		return err
	}

	// 3. Check socket_vmnet.
	if err := checkOrInstallSocketVmnet(ctx, runner, host, autoInstall); err != nil {
		return err
	}

	// 4. Check sudoers for Lima.
	if err := ensureSudoers(ctx, runner, host); err != nil {
		return err
	}

	// 5. Ensure socket_vmnet service is running.
	if err := ensureSocketVmnetRunning(ctx, runner, host); err != nil {
		return err
	}

	slog.Info("all prerequisites satisfied", "host", host)
	fmt.Printf("  [%s] ✅ All prerequisites satisfied\n", host)
	return nil
}

func checkOrInstallBrew(ctx context.Context, runner *remote.Runner, host string, autoInstall bool) error {
	_, err := runner.Run(ctx, "which brew || ls /opt/homebrew/bin/brew 2>/dev/null || ls /usr/local/bin/brew 2>/dev/null")
	if err == nil {
		slog.Debug("homebrew found", "host", host)
		return nil
	}

	if !autoInstall {
		return fmt.Errorf("[%s] homebrew is not installed (use --auto-install to install automatically)", host)
	}

	slog.Info("installing homebrew", "host", host)
	fmt.Printf("  [%s] Installing Homebrew...\n", host)
	_, err = runner.RunShell(ctx, `/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"`)
	if err != nil {
		return fmt.Errorf("[%s] failed to install homebrew: %w", host, err)
	}
	return nil
}

func checkOrInstallLima(ctx context.Context, runner *remote.Runner, host string, autoInstall bool) error {
	out, err := runner.Run(ctx, "limactl --version 2>/dev/null")
	if err == nil && out != "" {
		slog.Debug("lima found", "host", host, "version", strings.TrimSpace(out))
		return nil
	}

	if !autoInstall {
		return fmt.Errorf("[%s] lima is not installed (use --auto-install to install automatically)", host)
	}

	slog.Info("installing lima", "host", host)
	fmt.Printf("  [%s] Installing Lima...\n", host)
	_, err = runner.Run(ctx, "brew install lima")
	if err != nil {
		return fmt.Errorf("[%s] failed to install lima: %w", host, err)
	}
	return nil
}

func checkOrInstallSocketVmnet(ctx context.Context, runner *remote.Runner, host string, autoInstall bool) error {
	_, err := runner.Run(ctx, "brew list socket_vmnet &>/dev/null")
	if err == nil {
		slog.Debug("socket_vmnet found", "host", host)
		return nil
	}

	if !autoInstall {
		return fmt.Errorf("[%s] socket_vmnet is not installed (use --auto-install to install automatically)", host)
	}

	slog.Info("installing socket_vmnet", "host", host)
	fmt.Printf("  [%s] Installing socket_vmnet...\n", host)
	_, err = runner.Run(ctx, "brew install socket_vmnet")
	if err != nil {
		return fmt.Errorf("[%s] failed to install socket_vmnet: %w", host, err)
	}
	return nil
}

func ensureSudoers(ctx context.Context, runner *remote.Runner, host string) error {
	// Check if the sudoers file already exists.
	_, err := runner.Run(ctx, "test -f /etc/sudoers.d/lima && echo exists")
	if err == nil {
		slog.Debug("lima sudoers already configured", "host", host)
		return nil
	}

	slog.Info("configuring lima sudoers", "host", host)
	fmt.Printf("  [%s] Configuring Lima sudoers...\n", host)
	_, err = runner.RunShell(ctx, "limactl sudoers | sudo tee /etc/sudoers.d/lima >/dev/null")
	if err != nil {
		return fmt.Errorf("[%s] failed to configure sudoers: %w", host, err)
	}
	return nil
}

func ensureSocketVmnetRunning(ctx context.Context, runner *remote.Runner, host string) error {
	// Check if the socket exists.
	_, err := runner.Run(ctx, "test -S /var/run/socket_vmnet")
	if err == nil {
		slog.Debug("socket_vmnet running", "host", host)
		return nil
	}

	slog.Info("starting socket_vmnet service", "host", host)
	fmt.Printf("  [%s] Starting socket_vmnet service...\n", host)
	_, err = runner.Run(ctx, "sudo brew services start socket_vmnet")
	if err != nil {
		return fmt.Errorf("[%s] failed to start socket_vmnet: %w", host, err)
	}
	return nil
}

func newRunner(node config.NodeConfig) *remote.Runner {
	if node.SSHUser != "" || node.SSHPort != "" || node.SSHKeyPath != "" {
		return remote.NewRunnerWithOpts(node.Host, node.SSHUser, node.SSHPort, node.SSHKeyPath)
	}
	return remote.NewRunner(node.Host)
}
