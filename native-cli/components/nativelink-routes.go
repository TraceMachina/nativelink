package components

import (
	_ "embed"
	"fmt"

	"github.com/pulumi/pulumi-kubernetes/sdk/v4/go/kubernetes/yaml"
	"github.com/pulumi/pulumi/sdk/v3/go/pulumi"
)

type NativeLinkRoutes struct {
	Dependencies []pulumi.Resource
}

// These are vendored yaml files which we don't port to Pulumi so that we can
// potentially adjust/reuse them in more generic contexts. We embed them in the
// executable to keep the cli portable.
//
//go:embed embedded/nativelink-routes.yaml
var nativeLinkRoutesYaml string

// Install sets up the Routes for the NativeLink deployment.
//
// Contrary to the rest of the NativeLink setup, these gateways aren't part of
// the regular deployment. Recreating the Gateways would change their local IPs
// which makes development less convenient. Instead, we create them once and
// take the IPs for granted to be fixed after initial creation.
//
// It's unclear whether this indirection is the right approach and we might add
// them to the regular deployments when more infrastructure is in place to
// support changing Gateway IPs.
func (component *NativeLinkRoutes) Install(
	ctx *pulumi.Context,
	name string,
) ([]pulumi.Resource, error) {
	NativeLinkRoutes, err := yaml.NewConfigGroup(
		ctx,
		name,
		&yaml.ConfigGroupArgs{
			YAML: []string{nativeLinkRoutesYaml},
		},
		pulumi.DependsOn(component.Dependencies),
	)
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	return []pulumi.Resource{NativeLinkRoutes}, nil
}
