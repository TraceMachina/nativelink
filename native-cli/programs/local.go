package programs

import (
	"log"
	"os"
	"slices"

	"github.com/TraceMachina/nativelink/native-cli/components"
	"github.com/pulumi/pulumi/sdk/v3/go/pulumi"
)

// ProgramForLocalCluster is the Pulumi program for the development cluster.
//
//nolint:funlen
func ProgramForLocalCluster(ctx *pulumi.Context) error {
	components.Check(components.AddComponent(
		ctx,
		"kind-registry",
		&components.Registry{
			InternalPort: 5000, //nolint:mnd
			ExternalPort: 5001, //nolint:mnd
		},
	))

	cilium, err := components.AddComponent(
		ctx,
		"cilium",
		&components.Cilium{Version: "1.16.0-pre.2"},
	)
	if err != nil {
		log.Println(err)
		os.Exit(1)
	}

	components.Check(components.AddComponent(
		ctx,
		"local-sources",
		&components.LocalPVAndPVC{
			Size:     "50Mi",
			HostPath: "/mnt",
		},
	))

	components.Check(components.AddComponent(
		ctx,
		"nix-store",
		&components.LocalPVAndPVC{
			Size:     "10Gi",
			HostPath: "/nix",
		},
	))

	tektonOperator, err := components.AddComponent(
		ctx,
		"tekton-operator",
		&components.TektonOperator{
			Version: "0.72.0",
			Dependencies: slices.Concat(
				cilium,
			),
		},
	)
	if err != nil {
		log.Println(err)
		os.Exit(1)
	}

	flux, err := components.AddComponent(
		ctx,
		"flux",
		&components.Flux{Version: "2.3.0"},
	)
	if err != nil {
		log.Println(err)
		os.Exit(1)
	}

	components.Check(components.AddComponent(
		ctx,
		"capacitor",
		&components.Capacitor{
			Dependencies: slices.Concat(
				flux,
			),
		},
	))

	nativeLinkGateways, err := components.AddComponent(
		ctx,
		"nativelink-gateways",
		&components.NativeLinkGateways{
			Dependencies: slices.Concat(
				cilium,
				flux,
				tektonOperator,
			),
		},
	)
	if err != nil {
		log.Println(err)
		os.Exit(1)
	}

	nativeLinkRoutes, err := components.AddComponent(
		ctx,
		"nativelink-routes",
		&components.NativeLinkRoutes{
			Dependencies: nativeLinkGateways,
		},
	)
	if err != nil {
		log.Println(err)
		os.Exit(1)
	}

	hubbleGateway := components.Gateway{
		ExternalPort: 8080, //nolint:mnd
		InternalPort: 80,   //nolint:mnd
		Routes: []components.RouteConfig{
			{
				Prefix:  "/",
				Cluster: "hubble-gateway",
			},
		},
	}

	tknGateway := components.Gateway{
		ExternalPort: 8081, //nolint:mnd
		InternalPort: 8080, //nolint:mnd
		Routes: []components.RouteConfig{
			{
				Prefix:  "/",
				Cluster: "tkn-gateway",
			},
		},
	}

	capacitorGateway := components.Gateway{
		ExternalPort: 9000, //nolint:mnd
		InternalPort: 9000, //nolint:mnd
		Routes: []components.RouteConfig{
			{
				Prefix:  "/",
				Cluster: "capacitor-gateway",
			},
		},
	}

	nativelinkGateway := components.Gateway{
		ExternalPort: 8082, //nolint:mnd
		InternalPort: 8089, //nolint:mnd
		Routes: []components.RouteConfig{
			{
				Prefix:  "/eventlistener",
				Cluster: "el-gateway",
			},
			// Add grpc proxy support in future.
			// {
			// 	Prefix: "/cache",
			// 	Cluster:       "cache-gateway",
			// 	PrefixRewrite: "/",
			// 	GRPC: false,
			// },
			// {
			// 	Prefix: "/scheduler",
			// 	Cluster:       "scheduler-gateway",
			// 	PrefixRewrite: "/",
			// 	GRPC: false,
			// },
		},
	}

	components.Check(components.AddComponent(
		ctx,
		"kind-loadbalancer",
		&components.Loadbalancer{
			Gateways: []components.Gateway{
				capacitorGateway,
				nativelinkGateway,
				hubbleGateway,
				tknGateway,
			},
			Dependencies: slices.Concat(
				nativeLinkRoutes,
			),
		},
	))

	return nil
}
