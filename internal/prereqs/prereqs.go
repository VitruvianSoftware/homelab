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

// brewCmd returns the full path to brew, auto-detecting ARM64 vs Intel.
const brewDetect = `if [ -x /opt/homebrew/bin/brew ]; then echo /opt/homebrew/bin/brew; elif [ -x /usr/local/bin/brew ]; then echo /usr/local/bin/brew; else echo ""; fi`

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

	// 1. Detect or install Homebrew, get its full path.
	brewPath, err := detectOrInstallBrew(ctx, runner, host, autoInstall)
	if err != nil {
		return err
	}

	// 2. Check/install Lima (using full brew path).
	if err := checkOrInstallLima(ctx, runner, host, brewPath, autoInstall); err != nil {
		return err
	}

	// 3. Check/install socket_vmnet (using full brew path).
	if err := checkOrInstallSocketVmnet(ctx, runner, host, brewPath, autoInstall); err != nil {
		return err
	}

	// 4. Determine bin prefix for limactl (brew --prefix).
	binPrefix, _ := runner.RunShell(ctx, fmt.Sprintf("%s --prefix", brewPath))
	binPrefix = strings.TrimSpace(binPrefix)
	if binPrefix == "" {
		binPrefix = "/opt/homebrew" // sensible default
	}
	limactlPath := binPrefix + "/bin/limactl"

	// 5. Check sudoers for Lima.
	if err := ensureSudoers(ctx, runner, host, limactlPath); err != nil {
		return err
	}

	// 6. Ensure socket_vmnet service is running.
	if err := ensureSocketVmnetRunning(ctx, runner, host, brewPath); err != nil {
		return err
	}

	// 7. Ensure socket_vmnet directory is readable by Lima.
	_, _ = runner.RunShell(ctx, fmt.Sprintf("sudo chmod 755 $(%s --prefix)/var/run 2>/dev/null", brewPath))

	slog.Info("all prerequisites satisfied", "host", host)
	fmt.Printf("  [%s] ✅ All prerequisites satisfied\n", host)
	return nil
}

func detectOrInstallBrew(ctx context.Context, runner *remote.Runner, host string, autoInstall bool) (string, error) {
	out, err := runner.RunShell(ctx, brewDetect)
	if err == nil {
		brewPath := strings.TrimSpace(out)
		if brewPath != "" {
			slog.Debug("homebrew found", "host", host, "path", brewPath)
			return brewPath, nil
		}
	}

	if !autoInstall {
		return "", fmt.Errorf("[%s] homebrew is not installed (use --auto-install to install automatically)", host)
	}

	slog.Info("installing homebrew", "host", host)
	fmt.Printf("  [%s] Installing Homebrew...\n", host)
	_, err = runner.RunShell(ctx, `NONINTERACTIVE=1 /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"`)
	if err != nil {
		return "", fmt.Errorf("[%s] failed to install homebrew: %w", host, err)
	}

	// Re-detect after install.
	out, err = runner.RunShell(ctx, brewDetect)
	if err != nil {
		return "", fmt.Errorf("[%s] homebrew installed but not detected: %w", host, err)
	}
	return strings.TrimSpace(out), nil
}

func checkOrInstallLima(ctx context.Context, runner *remote.Runner, host, brewPath string, autoInstall bool) error {
	// Check using the brew prefix path for limactl.
	prefix, _ := runner.RunShell(ctx, fmt.Sprintf("%s --prefix", brewPath))
	prefix = strings.TrimSpace(prefix)
	if prefix == "" {
		prefix = "/opt/homebrew"
	}
	limactlPath := prefix + "/bin/limactl"

	out, err := runner.RunShell(ctx, fmt.Sprintf("%s --version 2>/dev/null", limactlPath))
	if err == nil && out != "" {
		slog.Debug("lima found", "host", host, "version", strings.TrimSpace(out))
		return nil
	}

	if !autoInstall {
		return fmt.Errorf("[%s] lima is not installed (use --auto-install to install automatically)", host)
	}

	slog.Info("installing lima", "host", host)
	fmt.Printf("  [%s] Installing Lima...\n", host)
	_, err = runner.RunShell(ctx, fmt.Sprintf("%s install lima", brewPath))
	if err != nil {
		return fmt.Errorf("[%s] failed to install lima: %w", host, err)
	}
	return nil
}

func checkOrInstallSocketVmnet(ctx context.Context, runner *remote.Runner, host, brewPath string, autoInstall bool) error {
	_, err := runner.RunShell(ctx, fmt.Sprintf("%s list socket_vmnet &>/dev/null", brewPath))
	if err == nil {
		slog.Debug("socket_vmnet found", "host", host)
		return nil
	}

	if !autoInstall {
		return fmt.Errorf("[%s] socket_vmnet is not installed (use --auto-install to install automatically)", host)
	}

	slog.Info("installing socket_vmnet", "host", host)
	fmt.Printf("  [%s] Installing socket_vmnet...\n", host)
	_, err = runner.RunShell(ctx, fmt.Sprintf("%s install socket_vmnet", brewPath))
	if err != nil {
		return fmt.Errorf("[%s] failed to install socket_vmnet: %w", host, err)
	}
	return nil
}

func ensureSudoers(ctx context.Context, runner *remote.Runner, host, limactlPath string) error {
	// Check if the sudoers file already exists.
	_, err := runner.Run(ctx, "test -f /etc/sudoers.d/lima && echo exists")
	if err == nil {
		slog.Debug("lima sudoers already configured", "host", host)
		return nil
	}

	slog.Info("configuring lima sudoers", "host", host)
	fmt.Printf("  [%s] Configuring Lima sudoers...\n", host)
	_, err = runner.RunShell(ctx, fmt.Sprintf("%s sudoers | sudo tee /etc/sudoers.d/lima >/dev/null", limactlPath))
	if err != nil {
		return fmt.Errorf("[%s] failed to configure sudoers: %w", host, err)
	}
	return nil
}

func ensureSocketVmnetRunning(ctx context.Context, runner *remote.Runner, host, brewPath string) error {
	// Check common socket locations (use sudo since directories may have restricted perms).
	socketPaths := []string{
		"/opt/homebrew/var/run/socket_vmnet",
		"/usr/local/var/run/socket_vmnet",
		"/var/run/socket_vmnet",
	}
	for _, p := range socketPaths {
		_, err := runner.RunShell(ctx, fmt.Sprintf("sudo test -S %s", p))
		if err == nil {
			slog.Debug("socket_vmnet running", "host", host, "path", p)
			return nil
		}
	}

	// Also check if the service is already loaded and running via launchctl.
	_, err := runner.RunShell(ctx, "sudo launchctl list | grep -q socket_vmnet")
	if err == nil {
		slog.Debug("socket_vmnet service loaded (socket check may have failed due to permissions)", "host", host)
		return nil
	}

	slog.Info("starting socket_vmnet service", "host", host)
	fmt.Printf("  [%s] Starting socket_vmnet service...\n", host)
	_, err = runner.RunShell(ctx, fmt.Sprintf("sudo %s services start socket_vmnet", brewPath))
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
