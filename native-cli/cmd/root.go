package cmd

import (
	"os"

	"github.com/spf13/cobra"
)

// NewRootCmd constructs a new root command.
func NewRootCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "native",
		Short: "NativeLink CLI",
		Long: `The NativeLink CLI lets you test NativeLink in a local Kubernetes setup.

For users this tool showcases running NativeLink in Kubernetes.

This demo cluster is aggressively optimized for rapid development of NativeLink
itself. It implements its own internal CI system which leverages cache reuse
between of the local nix store so that out-of-cluster nativelink builds become
immediately available to in-cluster deployments.`,
	}
}

// Execute adds all child commands to the root command and sets flags appropriately.
// This is called by main.main(). It only needs to happen once to the rootCmd.
func Execute() {
	rootCmd := NewRootCmd()
	rootCmd.AddCommand(NewUpCmd())
	rootCmd.AddCommand(NewDownCmd())

	if err := rootCmd.Execute(); err != nil {
		os.Exit(1)
	}
}
