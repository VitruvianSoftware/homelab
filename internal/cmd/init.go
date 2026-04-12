package cmd

import (
	"fmt"

	"github.com/spf13/cobra"

	"github.com/VitruvianSoftware/homelab/internal/config"
	"github.com/VitruvianSoftware/homelab/internal/cluster"
)

func newInitCmd(configFile *string) *cobra.Command {
	return &cobra.Command{
		Use:   "init",
		Short: "Bootstrap a new cluster from the config file",
		Long: `Initialize a new Kubernetes cluster by provisioning Lima VMs on each
configured host and bootstrapping K3s in HA mode.

This command is idempotent: it will skip any steps that have already been completed.`,
		RunE: func(cmd *cobra.Command, args []string) error {
			cfg, err := config.Load(*configFile)
			if err != nil {
				return fmt.Errorf("loading config: %w", err)
			}
			return cluster.Init(cmd.Context(), cfg)
		},
	}
}
