package cmd

import (
	"fmt"

	"github.com/spf13/cobra"

	"github.com/VitruvianSoftware/homelab/internal/config"
	"github.com/VitruvianSoftware/homelab/internal/cluster"
)

func newRemoveCmd(configFile *string) *cobra.Command {
	var nodeName string

	cmd := &cobra.Command{
		Use:   "remove <node-host>",
		Short: "Drain and remove a node from the cluster",
		Long: `Remove drains the specified node, uninstalls K3s, and optionally
destroys the Lima VM on the target host.`,
		Args: cobra.ExactArgs(1),
		RunE: func(c *cobra.Command, args []string) error {
			nodeName = args[0]
			cfg, err := config.Load(*configFile)
			if err != nil {
				return fmt.Errorf("loading config: %w", err)
			}
			return cluster.Remove(c.Context(), cfg, nodeName)
		},
	}

	return cmd
}
