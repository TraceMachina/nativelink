package cmd

import (
	"log"
	"os"

	"github.com/TraceMachina/nativelink/native-cli/clusters"
	"github.com/spf13/cobra"
	"sigs.k8s.io/kind/pkg/cluster"
	kindcmd "sigs.k8s.io/kind/pkg/cmd"
)

// NewDownCmd constructs a new down command.
func NewDownCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "down",
		Short: "Immediately destroy the cluster",
		Long: `Immediately tear down the cluster by deleting the kind nodes.

This command doesn't gracefully shutdown. Instead, removes the kind nodes. Since
the cluster is self-contained this is safe to do and faster than a graceful
shutdown.`,
		Run: func(_ *cobra.Command, _ []string) {
			kindProvider := cluster.NewProvider(
				cluster.ProviderWithLogger(kindcmd.NewLogger()),
			)
			if err := clusters.DeleteLocalCluster(kindProvider); err != nil {
				log.Println(err)
				os.Exit(1)
			}
			os.Exit(0)
		},
	}
}
