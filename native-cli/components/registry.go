package components

import (
	"fmt"

	"github.com/pulumi/pulumi-docker/sdk/v3/go/docker"
	"github.com/pulumi/pulumi/sdk/v3/go/pulumi"
)

// A local container registry.
type Registry struct {
	InternalPort int
	ExternalPort int
}

// Install installs a local registry on the host and makes it available to a
// kind cluster.
func (component *Registry) Install(
	ctx *pulumi.Context,
	name string,
) ([]pulumi.Resource, error) {
	registry, err := docker.NewContainer(
		ctx,
		name,
		&docker.ContainerArgs{
			Image: pulumi.String("registry:2"),
			Ports: docker.ContainerPortArray{
				&docker.ContainerPortArgs{
					Internal: pulumi.Int(component.InternalPort),
					External: pulumi.Int(component.ExternalPort),
				},
			},
			NetworksAdvanced: docker.ContainerNetworksAdvancedArray{
				// By default the registry will connect to the `bridge` network.
				// We add it to the `kind` network as well so that we can push
				// images locally to the registry and have them accessible by
				// kind.
				&docker.ContainerNetworksAdvancedArgs{
					Name: pulumi.String("kind"),
				},
			},
			Restart: pulumi.String("always"),
			Name:    pulumi.String("kind-registry"),
		},
	)
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	return []pulumi.Resource{registry}, nil
}
