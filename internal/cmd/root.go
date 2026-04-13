package cmd

import (
	"fmt"

	"github.com/spf13/cobra"

	"github.com/VitruvianSoftware/homelab/internal/logging"
)

// NewRootCmd creates the top-level CLI command.
func NewRootCmd(version, commit, date string) *cobra.Command {
	var (
		configFile string
		verbose    bool
	)

	root := &cobra.Command{
		Use:   "homelab",
		Short: "Declaratively provision and manage multi-node K8s homelab clusters on macOS",
		Long: `homelab is a CLI tool for provisioning and managing Kubernetes clusters
across multiple macOS machines using Lima VZ and K3s.

Define your cluster topology in a YAML config file, then use homelab to
bootstrap, scale, diagnose, and tear down your cluster.`,
		SilenceUsage:  true,
		SilenceErrors: true,
		PersistentPreRun: func(cmd *cobra.Command, args []string) {
			logging.Setup(verbose)
		},
	}

	root.PersistentFlags().StringVarP(&configFile, "config", "c", "homelab.yaml", "path to cluster config file")
	root.PersistentFlags().BoolVarP(&verbose, "verbose", "v", false, "enable debug logging")

	root.AddCommand(
		newInitCmd(&configFile),
		newApplyCmd(&configFile),
		newJoinCmd(&configFile),
		newRemoveCmd(&configFile),
		newDestroyCmd(&configFile),
		newUpgradeCmd(&configFile),
		newBackupCmd(&configFile),
		newRestoreCmd(&configFile),
		newDoctorCmd(&configFile),
		newStatusCmd(&configFile),
		newVersionCmd(version, commit, date),
	)

	return root
}

func newVersionCmd(version, commit, date string) *cobra.Command {
	return &cobra.Command{
		Use:   "version",
		Short: "Print the version information",
		Run: func(cmd *cobra.Command, args []string) {
			fmt.Printf("homelab %s (commit: %s, built: %s)\n", version, commit, date)
		},
	}
}
