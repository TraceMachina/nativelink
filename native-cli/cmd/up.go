package cmd

import (
	"context"
	"log"
	"os"

	"github.com/TraceMachina/nativelink/native-cli/clusters"
	"github.com/TraceMachina/nativelink/native-cli/programs"
	"github.com/pulumi/pulumi/sdk/v3/go/auto"
	"github.com/pulumi/pulumi/sdk/v3/go/auto/optup"
	"github.com/spf13/cobra"
	"sigs.k8s.io/kind/pkg/cluster"
	kindcmd "sigs.k8s.io/kind/pkg/cmd"
)

// runUpCmd implements the Run function of the up command.
func runUpCmd(_ *cobra.Command, _ []string) {
	kindProvider := cluster.NewProvider(
		cluster.ProviderWithLogger(kindcmd.NewLogger()),
	)

	//nolint:gomnd
	if err := clusters.CreateLocalCluster(kindProvider, 5000, 5001); err != nil {
		log.Println(err)
		log.Println("Skipping kind cluster creation")
	}

	ctx := context.Background()
	stackName := auto.FullyQualifiedStackName(
		"organization",
		"nativelink",
		"dev",
	)

	// Only use this for local development.
	envvars := auto.EnvVars(
		map[string]string{"PULUMI_CONFIG_PASSPHRASE": ""},
	)

	workDir := "."

	stack, err := auto.UpsertStackLocalSource(
		ctx,
		stackName,
		workDir,
		auto.Program(programs.ProgramForLocalCluster),
		envvars,
	)
	if err != nil {
		log.Println(err)
		os.Exit(1)
	}

	log.Printf("Operating on stack %q. Refreshing...\n", stackName)

	_, err = stack.Refresh(ctx)
	if err != nil {
		log.Println(err)
		os.Exit(1)
	}

	log.Println("Refresh succeeded. Updating stack...")

	stdoutStreamer := optup.ProgressStreams(os.Stdout)

	_, err = stack.Up(ctx, stdoutStreamer)
	if err != nil {
		log.Println(err)
		os.Exit(1)
	}

	log.Println(
		"Cluster is running. Use `kubectl` to interact with it.",
	)
}

// NewUpCmd constructs a new up command.
func NewUpCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "up",
		Short: "Start the development cluster",
		Long: `Start a kind cluster with the following configuration:

- A control pland and three worker nodes.
- A container registry which runs on the host and is passed through to the kind
  cluster.
- Cilium as CNI with a configuration that allows creating Gateways with the
  "cilium" gatewayClassName. The Gateways map to IPs on the host's local
  container network.
- Passthrough from the git root of the current repository to the kind nodes and
  then into the cluster via a PersistentVolume and PersistentVolumeClaim called
  "local-sources-pv" and "local-sources-pvc".
- Passthrough from the hosts "/nix/store" into the kind nodes and then into
  the cluster via a PersistentVolume and PersistentVolumeClaim called
  "nix-store-pv" and "nix-store-pvc".

If this command is called when the cluster is already running the cluster
creation is skipped and instead only a refresh of the deployments is applied.

All resources apart from the kind nodes themselves are managed by Pulumi. You
can invoke "pulumi stack" with an empty password to get the resource tree.
`,
		Run: runUpCmd,
	}
}
