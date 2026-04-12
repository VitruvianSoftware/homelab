package cmd

import (
	"fmt"

	"github.com/spf13/cobra"

	"github.com/VitruvianSoftware/homelab/internal/config"
	"github.com/VitruvianSoftware/homelab/internal/cluster"
)

func newJoinCmd(configFile *string) *cobra.Command {
	return &cobra.Command{
		Use:   "join",
		Short: "Add new node(s) to an existing cluster",
		Long: `Join adds nodes that are defined in the config but not yet part of the cluster.
It detects which nodes are missing and provisions only those.`,
		RunE: func(cmd *cobra.Command, args []string) error {
			cfg, err := config.Load(*configFile)
			if err != nil {
				return fmt.Errorf("loading config: %w", err)
			}
			return cluster.Join(cmd.Context(), cfg)
		},
	}
}
