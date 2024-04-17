package components

import (
	"context"
	"fmt"
	"log"
	"regexp"
	"slices"

	"github.com/docker/docker/api/types"
	"github.com/docker/docker/client"
	"github.com/pulumi/pulumi-kubernetes/sdk/v4/go/kubernetes/apiextensions"
	helmv3 "github.com/pulumi/pulumi-kubernetes/sdk/v4/go/kubernetes/helm/v3"
	metav1 "github.com/pulumi/pulumi-kubernetes/sdk/v4/go/kubernetes/meta/v1"
	"github.com/pulumi/pulumi-kubernetes/sdk/v4/go/kubernetes/yaml"
	"github.com/pulumi/pulumi/sdk/v3/go/pulumi"
)

// Configuration for a Cilium deployment.
type Cilium struct {
	Version string
}

// Install installs Cilium on the cluster.
func (component *Cilium) Install(
	ctx *pulumi.Context,
	name string,
) ([]pulumi.Resource, error) {
	gatewayAPI, err := yaml.NewConfigFile(ctx, name, &yaml.ConfigFileArgs{
		File: "https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.0.0/experimental-install.yaml",
	})
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	cilium, err := helmv3.NewRelease(ctx, name, &helmv3.ReleaseArgs{
		Chart:     pulumi.String("cilium"),
		Version:   pulumi.String(component.Version),
		Namespace: pulumi.String("kube-system"),
		RepositoryOpts: helmv3.RepositoryOptsArgs{
			Repo: pulumi.String("https://helm.cilium.io/"),
		},
		Values: pulumi.Map{
			// Name of the `control-plane` node in `kubectl get nodes`.
			"k8sServiceHost": pulumi.String("kind-control-plane"),

			// Forwarded port in `docker ps` for the control plane.
			"k8sServicePort": pulumi.String("6443"),

			// Required for proper Cilium operation.
			"kubeProxyReplacement": pulumi.String("strict"),

			// Use the Gateway API instead of the older Ingress resource.
			"gatewayAPI": pulumi.Map{"enabled": pulumi.Bool(true)},

			// Use L2-IPAM.
			"l2announcements": pulumi.Map{"enabled": pulumi.Bool(true)},

			"image": pulumi.Map{"pullPolicy": pulumi.String("IfNotPresent")},

			"hubble": pulumi.Map{
				"relay": pulumi.Map{"enabled": pulumi.Bool(true)},
				"ui":    pulumi.Map{"enabled": pulumi.Bool(true)},
			},
		},
	}, pulumi.DependsOn([]pulumi.Resource{gatewayAPI}))
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	l2Announcements, err := l2Announcements(ctx, []pulumi.Resource{cilium})
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	defaultPool, err := defaultPool(ctx, []pulumi.Resource{cilium})
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	return slices.Concat(
		[]pulumi.Resource{cilium},
		l2Announcements,
		defaultPool,
	), nil
}

// l2Announcements creates the CiliumL2AnnouncementPolicy for the cluster.
func l2Announcements(
	ctx *pulumi.Context,
	ciliumResources []pulumi.Resource,
) ([]pulumi.Resource, error) {
	l2Announcements, err := apiextensions.NewCustomResource(
		ctx,
		"l2-announcements",
		&apiextensions.CustomResourceArgs{
			ApiVersion: pulumi.String("cilium.io/v2alpha1"),
			Kind:       pulumi.String("CiliumL2AnnouncementPolicy"),
			// Metadata...
			Metadata: &metav1.ObjectMetaArgs{
				Name: pulumi.StringPtr("l2-announcements"),
			},

			OtherFields: map[string]interface{}{
				"spec": pulumi.Map{
					"externalIPs":     pulumi.Bool(true),
					"loadBalancerIPs": pulumi.Bool(true),
				},
			},
		},
		pulumi.DependsOn(ciliumResources),
	)
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	return []pulumi.Resource{l2Announcements}, nil
}

// kindCIDRs returns the container id range of the kind network.
func kindCIDRs() (string, error) {
	dockerCtx := context.Background()

	cli, err := client.NewClientWithOpts(
		client.FromEnv,
		client.WithAPIVersionNegotiation(),
	)
	if err != nil {
		return "", fmt.Errorf("%w: %w", errPulumi, err)
	}

	networks, err := cli.NetworkList(dockerCtx, types.NetworkListOptions{})
	if err != nil {
		return "", fmt.Errorf("%w: %w", errPulumi, err)
	}

	for _, network := range networks {
		if network.Name == "kind" {
			if len(network.IPAM.Config) > 0 {
				kindNetCIDR := network.IPAM.Config[0].Subnet

				return kindNetCIDR, nil
			}
		}
	}

	return "", fmt.Errorf("%w: %s", errPulumi, "no kind network found")
}

// defaultPool creates a CiliumLoadBalancerIPPool which allocates IPs on the
// local kind network which is available to the host. Usually this will be
// something like the 172.20.255.x ip range from the docker network.
func defaultPool(
	ctx *pulumi.Context,
	ciliumResources []pulumi.Resource,
) ([]pulumi.Resource, error) {
	kindNetCIDR, err := kindCIDRs()
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	// This regex replaces the last octet block from "0.0/16" to "255.0/28".
	re := regexp.MustCompile(`0\.0/16$`)
	ciliumIPCIDR := re.ReplaceAllString(kindNetCIDR, "255.0/28")

	log.Println("KIND Network CIDR:", kindNetCIDR)
	log.Println("Modified CIDR for Cilium:", ciliumIPCIDR)

	defaultPool, err := apiextensions.NewCustomResource(
		ctx,
		"default-pool",
		&apiextensions.CustomResourceArgs{
			ApiVersion: pulumi.String("cilium.io/v2alpha1"),
			Kind:       pulumi.String("CiliumLoadBalancerIPPool"),
			Metadata: &metav1.ObjectMetaArgs{
				Name: pulumi.StringPtr("default-pool"),
			},
			OtherFields: map[string]interface{}{
				"spec": pulumi.Map{
					"cidrs": pulumi.Array{
						pulumi.Map{
							"cidr": pulumi.String(ciliumIPCIDR),
						},
					},
				},
			},
		},
		pulumi.DependsOn(ciliumResources),
	)
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	return []pulumi.Resource{defaultPool}, nil
}
