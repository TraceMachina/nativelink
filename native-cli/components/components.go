package components

import (
	"errors"
	"fmt"
	"log"
	"os"

	"github.com/pulumi/pulumi/sdk/v3/go/pulumi"
)

var (
	errPulumi    = errors.New("pulumi error")
	errComponent = errors.New("component error")
)

// Check may be usedd as convenience wrapper to error-check component creation.
func Check(_ []pulumi.Resource, err error) {
	if err != nil {
		log.Println(err)
		os.Exit(1)
	}
}

// A generic Component type used by all components.
type Component interface {
	Install(ctx *pulumi.Context, name string) ([]pulumi.Resource, error)
}

// AddComponent adds a component to the cluster.
func AddComponent[C Component](
	ctx *pulumi.Context,
	name string,
	component C,
) ([]pulumi.Resource, error) {
	resources, err := component.Install(ctx, name)
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errComponent, err)
	}

	return resources, nil
}
