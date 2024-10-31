package components

import (
	_ "embed"
	"fmt"

	"github.com/pulumi/pulumi-kubernetes/sdk/v4/go/kubernetes/yaml"
	"github.com/pulumi/pulumi/sdk/v3/go/pulumi"
)

type Capacitor struct {
	Dependencies []pulumi.Resource
}

// These are vendored yaml files which we don't port to Pulumi so that we can
// potentially adjust/reuse them in more generic contexts. We embed them in the
// executable to keep the cli portable.
//
//go:embed embedded/capacitor.yaml
var capacitorYaml string

// Install sets up the Capacitor dashboard.
func (component *Capacitor) Install(
	ctx *pulumi.Context,
	name string,
) ([]pulumi.Resource, error) {
	capacitor, err := yaml.NewConfigGroup(
		ctx,
		name,
		&yaml.ConfigGroupArgs{
			YAML: []string{capacitorYaml},
		},
		pulumi.DependsOn(component.Dependencies),
	)
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	return []pulumi.Resource{capacitor}, nil
}
