package components

import (
	"fmt"
	"time"

	"github.com/pulumi/pulumi-kubernetes/sdk/v4/go/kubernetes/yaml"
	"github.com/pulumi/pulumi/sdk/v3/go/pulumi"
)

type TektonOperator struct {
	Version      string
	Dependencies []pulumi.Resource
}

// Install installs Tekton Operator on the cluster and applies the TektonConfig.
func (component *TektonOperator) Install(
	ctx *pulumi.Context,
	name string,
) ([]pulumi.Resource, error) {
	// URL for TektonConfig
	urlBase := "https://raw.githubusercontent.com/tektoncd/operator/"
	urlPath := "/config/crs/kubernetes/config/all/operator_v1alpha1_config_cr.yaml"

	//  Install Tekton Operator with transformations
	tektonOperator, err := yaml.NewConfigGroup(ctx, name, &yaml.ConfigGroupArgs{
		Files: []string{
			fmt.Sprintf(
				"https://storage.googleapis.com/tekton-releases/operator/previous/v%s/release.yaml",
				component.Version,
			),
			fmt.Sprintf(
				urlBase+"v%s"+urlPath,
				component.Version,
			),
		},
		Transformations: []yaml.Transformation{
			func(state map[string]interface{}, _ ...pulumi.ResourceOption) {
				if kind, kindExists := state["kind"].(string); kindExists &&
					kind == "TektonConfig" {
					spec, specExists := state["spec"].(map[string]interface{})
					if !specExists {
						return
					}
					pipeline, pipelineExists := spec["pipeline"].(map[string]interface{})
					if !pipelineExists {
						pipeline = map[string]interface{}{}
						spec["pipeline"] = pipeline
					}
					pipeline["disable-affinity-assistant"] = true
					pipeline["coschedule"] = "pipelineruns"
				}
			},
		},
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
	time.Sleep(60 * time.Second) //nolint:mnd

	return true
}
