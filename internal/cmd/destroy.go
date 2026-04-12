package cmd

import (
	"fmt"

	"github.com/spf13/cobra"

	"github.com/VitruvianSoftware/homelab/internal/config"
	"github.com/VitruvianSoftware/homelab/internal/cluster"
)

func newDestroyCmd(configFile *string) *cobra.Command {
	var force bool

	cmd := &cobra.Command{
		Use:   "destroy",
		Short: "Tear down the entire cluster",
		Long: `Destroy uninstalls K3s from all nodes, stops and deletes all Lima VMs,
and removes the exported kubeconfig. Use --force to skip confirmation.`,
		RunE: func(c *cobra.Command, args []string) error {
			cfg, err := config.Load(*configFile)
			if err != nil {
				return fmt.Errorf("loading config: %w", err)
			}
			return cluster.Destroy(c.Context(), cfg, force)
		},
	}

	cmd.Flags().BoolVar(&force, "force", false, "skip confirmation prompt")

	return cmd
}
