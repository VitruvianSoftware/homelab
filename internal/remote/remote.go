// Package remote provides an SSH-based executor for running commands on
// remote macOS hosts. It shells out to the system ssh binary to leverage
// the user's existing SSH config and agent.
package remote

import (
	"bytes"
	"context"
	"fmt"
	"os/exec"
	"strings"
)

// Runner executes commands on a remote host via SSH.
type Runner struct {
	Host string
}

// NewRunner creates a new remote command runner for the given SSH host.
func NewRunner(host string) *Runner {
	return &Runner{Host: host}
}

// Run executes a command on the remote host and returns the combined output.
func (r *Runner) Run(ctx context.Context, command string) (string, error) {
	cmd := exec.CommandContext(ctx, "ssh",
		"-o", "BatchMode=yes",
		"-o", "ConnectTimeout=10",
		"-o", "StrictHostKeyChecking=accept-new",
		r.Host,
		command,
	)

	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr

	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("ssh %s: %w\nstderr: %s", r.Host, err, strings.TrimSpace(stderr.String()))
	}

	return strings.TrimSpace(stdout.String()), nil
}

// RunShell executes a command via sh -c on the remote host.
func (r *Runner) RunShell(ctx context.Context, script string) (string, error) {
	return r.Run(ctx, fmt.Sprintf("sh -c %q", script))
}

// LimaShell executes a command inside a Lima VM on the remote host.
func (r *Runner) LimaShell(ctx context.Context, vmName, command string) (string, error) {
	return r.Run(ctx, fmt.Sprintf("limactl shell %s -- %s", vmName, command))
}

// LimaShellSudo executes a command as root inside a Lima VM on the remote host.
func (r *Runner) LimaShellSudo(ctx context.Context, vmName, command string) (string, error) {
	return r.Run(ctx, fmt.Sprintf("limactl shell %s -- sudo sh -c %q", vmName, command))
}
