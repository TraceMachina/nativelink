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

	localSources, err := components.AddComponent(
		ctx,
		"local-sources",
		&components.LocalPVAndPVC{
			Size:     "50Mi",
			HostPath: "/mnt",
		},
	)
	if err != nil {
		log.Println(err)
		os.Exit(1)
	}

	nixStore, err := components.AddComponent(
		ctx,
		"nix-store",
		&components.LocalPVAndPVC{
			Size:     "10Gi",
			HostPath: "/nix",
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

	tektonPipelines, err := components.AddComponent(
		ctx,
		"tekton-pipelines",
		&components.TektonPipelines{Version: "0.58.0"},
	)
	if err != nil {
		log.Println(err)
		os.Exit(1)
	}

	tektonTriggers, err := components.AddComponent(
		ctx,
		"tekton-triggers",
		&components.TektonTriggers{Version: "0.26.1"},
	)
	if err != nil {
		log.Println(err)
		os.Exit(1)
	}

	components.Check(components.AddComponent(
		ctx,
		"tekton-dashboard",
		&components.TektonDashboard{Version: "0.45.0"},
	))

	components.Check(components.AddComponent(
		ctx,
		"rebuild-nativelink",
		&components.RebuildNativeLink{
			Dependencies: slices.Concat(
				cilium,
				tektonPipelines,
				tektonTriggers,
				localSources,
				nixStore,
				flux,
			),
		},
	))

	nativeLinkGateways, err := components.AddComponent(
		ctx,
		"nativelink-gatways",
		&components.NativeLinkGateways{
			Dependencies: cilium,
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
