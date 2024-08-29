package components

import (
	"fmt"

	"github.com/pulumi/pulumi-kubernetes/sdk/v4/go/kubernetes/yaml"
	"github.com/pulumi/pulumi/sdk/v3/go/pulumi"
)

// The configuration for Tekton Pipelines.
type TektonPipelines struct {
	Version string
}

// Install installs Tekton Pipelines on the cluster.
//
// This function performs an additional feature-flag transformation to set
// `disable-affinity-assistant=true` and `coscheduling=pipelineruns`.
func (component *TektonPipelines) Install(
	ctx *pulumi.Context,
	name string,
) ([]pulumi.Resource, error) {
	// This transformation disables the affinity assistant switches coscheduling
	// from "workspace" to "pipelineruns". This allows us to use multiple PVCs
	// in the same task. Usually this is discouraged for storage locality
	// reasons, but since we're running on a local kind cluster that doesn't
	// matter to us.
	transformation := func(state map[string]interface{}, _ ...pulumi.ResourceOption) {
		if kind, kindExists := state["kind"].(string); kindExists &&
			kind == "ConfigMap" {
			metadata, metadataExists := state["metadata"].(map[string]interface{})
			if !metadataExists {
				return
			}

			name, nameExists := metadata["name"].(string)

			namespace, namespaceExists := metadata["namespace"].(string)

			if nameExists && namespaceExists && name == "feature-flags" &&
				namespace == "tekton-pipelines" {
				data, dataExists := state["data"].(map[string]interface{})
				if dataExists {
					data["disable-affinity-assistant"] = "true" // Your specific configuration change
					data["coschedule"] = "pipelineruns"
				}
			}
		}
	}

	tektonPipelines, err := yaml.NewConfigFile(ctx, name, &yaml.ConfigFileArgs{
		File: fmt.Sprintf(
			"https://storage.googleapis.com/tekton-releases/pipeline/previous/v%s/release.yaml",
			component.Version,
		),
		Transformations: []yaml.Transformation{transformation},
	})
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	return []pulumi.Resource{tektonPipelines}, nil
}

// The configuration for Tekton Triggers.
type TektonTriggers struct {
	Version      string
	Dependencies []pulumi.Resource
}

// Install installs the Tekton Triggers release and interceptors on the cluster.
func (component *TektonTriggers) Install(
	ctx *pulumi.Context,
	name string,
) ([]pulumi.Resource, error) {
	tektonTriggers, err := yaml.NewConfigFile(
		ctx,
		name,
		&yaml.ConfigFileArgs{
			File: fmt.Sprintf(
				"https://storage.googleapis.com/tekton-releases/triggers/previous/v%s/release.yaml",
				component.Version,
			),
		},
		pulumi.DependsOn(
			component.Dependencies,
		),
	)
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	tektonTriggersInterceptors, err := yaml.NewConfigFile(
		ctx,
		name+"-interceptors",
		&yaml.ConfigFileArgs{
			File: fmt.Sprintf(
				"https://storage.googleapis.com/tekton-releases/triggers/previous/v%s/interceptors.yaml",
				component.Version,
			),
		},
		pulumi.DependsOn(
			[]pulumi.Resource{tektonTriggers},
		),
	)
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	return []pulumi.Resource{tektonTriggers, tektonTriggersInterceptors}, nil
}

// Configuration for the Tekton Dashboard.
type TektonDashboard struct {
	Version      string
	Dependencies []pulumi.Resource
}

// Install installs the Tekton Dashboard on the cluster.
func (component *TektonDashboard) Install(
	ctx *pulumi.Context,
	name string,
) ([]pulumi.Resource, error) {
	tektonDashboard, err := yaml.NewConfigFile(
		ctx,
		name,
		&yaml.ConfigFileArgs{
			File: fmt.Sprintf(
				"https://storage.googleapis.com/tekton-releases/dashboard/previous/v%s/release.yaml",
				component.Version,
			),
		},
		pulumi.DependsOn(
			component.Dependencies,
		),
	)
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	return []pulumi.Resource{tektonDashboard}, nil
}
