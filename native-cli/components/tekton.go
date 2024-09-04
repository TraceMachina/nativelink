package components

import (
	_ "embed"
	"fmt"
	"time"

	"github.com/pulumi/pulumi-kubernetes/sdk/v4/go/kubernetes/yaml"
	"github.com/pulumi/pulumi/sdk/v3/go/pulumi"
)

// Embed the TektonConfig YAML file
//
//go:embed embedded/tekton-config.yaml
var tektonConfig string

type TektonOperator struct {
	Version      string
	Dependencies []pulumi.Resource
}

// Install installs Tekton Operator on the cluster and applies the TektonConfig.
func (component *TektonOperator) Install(
	ctx *pulumi.Context,
	name string,
) ([]pulumi.Resource, error) {
	//  Install Tekton Operator
	tektonOperator, err := yaml.NewConfigGroup(ctx, name, &yaml.ConfigGroupArgs{
		Files: []string{fmt.Sprintf(
			"https://storage.googleapis.com/tekton-releases/operator/previous/v%s/release.yaml",
			component.Version,
		)},
		YAML: []string{tektonConfig},
	}, pulumi.DependsOn(component.Dependencies))
	if err != nil {
		return nil, fmt.Errorf("failed to install Tekton Operator: %w", err)
	}

	ready := waitForTektonToBeReady()
	if !ready {
		return nil, fmt.Errorf("tekton webhook is not ready: %w", err)
	}

	return []pulumi.Resource{tektonOperator}, nil
}

func waitForTektonToBeReady() bool {
	// Simulate Tekton State Gathering
	time.Sleep(120 * time.Second) //nolint:mnd

	return true
}
