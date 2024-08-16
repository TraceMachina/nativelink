package components

import (
	"fmt"

	"github.com/pulumi/pulumi-kubernetes/sdk/v4/go/kubernetes/yaml"
	"github.com/pulumi/pulumi/sdk/v3/go/pulumi"
)

// The configuration for Flux.
type Flux struct {
	Version string
}

// Install sets up Flux in the cluster.
func (component *Flux) Install(
	ctx *pulumi.Context,
	name string,
) ([]pulumi.Resource, error) {
	flux, err := yaml.NewConfigFile(ctx, name, &yaml.ConfigFileArgs{
		File: fmt.Sprintf(
			"https://github.com/fluxcd/flux2/releases/download/v%s/install.yaml",
			component.Version,
		),
	})
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	return []pulumi.Resource{flux}, nil
}
