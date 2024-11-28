package components

import (
	"bytes"
	"context"
	_ "embed"
	"flag"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"sync"
	"text/template"
	"time"

	"github.com/pulumi/pulumi-docker/sdk/v3/go/docker"
	"github.com/pulumi/pulumi/sdk/v3/go/pulumi"
	apiv1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/client-go/rest"
	"k8s.io/client-go/tools/clientcmd"
	gatewayClient "sigs.k8s.io/gateway-api/pkg/client/clientset/versioned"
)

// Embed the envoy.template.yaml
//
//go:embed embedded/envoy.template.yaml
var envoyTemplate string

// Route struct representing a route with prefix, cluster, and optional gRPC field.
type RouteConfig struct {
	Prefix        string
	Cluster       string
	PrefixRewrite string
	GRPC          bool
}

type Gateway struct {
	ExternalPort int
	InternalPort int
	Routes       []RouteConfig
}

// A local loadbalancer.
type Loadbalancer struct {
	Gateways     []Gateway
	Dependencies []pulumi.Resource
}

// Gateway struct to hold name and IP.
type InternalGateway struct {
	Name  string
	IP    string
	HTTP2 bool
}

// GatewaysData struct to hold multiple Gateways for templating.
type InternalGatewaysData struct {
	InternalGateways []InternalGateway
	Gateways         []Gateway
}

// Populate the Envoy template with gateway data.
func populateEnvoyConfig(
	internalGateways []InternalGateway,
	gateways []Gateway,
) (string, error) {
	tmpl, err := template.New("envoyConfig").Parse(envoyTemplate)
	if err != nil {
		return "", fmt.Errorf("failed to parse Envoy template: %w", err)
	}

	gatewaysData := InternalGatewaysData{
		InternalGateways: internalGateways,
		Gateways:         gateways,
	}

	var populatedConfig bytes.Buffer
	if err := tmpl.Execute(&populatedConfig, gatewaysData); err != nil {
		return "", fmt.Errorf("failed to populate Envoy config: %w", err)
	}

	return populatedConfig.String(), nil
}

// pollGateways polls the Kubernetes API for the required gateways until they are all found.
func pollGateways(
	ctx context.Context,
	gatewayClientset *gatewayClient.Clientset,
	requiredGateways map[string]bool,
) []InternalGateway {
	var gatewaysList []InternalGateway

	var mutex sync.Mutex

	var waitGroup sync.WaitGroup

	// Map to track added gateways
	addedGateways := make(map[string]bool)

	for name := range requiredGateways {
		waitGroup.Add(1)
		// Assign the function to a variable and then invoke it in the go statement
		go func(name string) {
			defer waitGroup.Done()

			for {
				select {
				case <-ctx.Done():
					return
				default:
					if gatewayAdded(
						ctx,
						gatewayClientset,
						name,
						addedGateways,
						requiredGateways,
						&gatewaysList,
						&mutex,
					) {
						return
					}

					time.Sleep(10 * time.Second) //nolint:mnd
				}
			}
		}(name)
	}

	waitGroup.Wait()

	return gatewaysList
}

// Add all necessary gateways from requiredGateways.
func gatewayAdded(
	ctx context.Context,
	gatewayClientset *gatewayClient.Clientset,
	name string,
	addedGateways map[string]bool,
	requiredGateways map[string]bool,
	gatewaysList *[]InternalGateway,
	mutex *sync.Mutex,
) bool {
	gateways, err := gatewayClientset.GatewayV1beta1().
		Gateways("").
		List(ctx, apiv1.ListOptions{})
	if err != nil {
		return false
	}

	mutex.Lock()
	defer mutex.Unlock()

	for _, gateway := range gateways.Items {
		if gateway.Name == name &&
			len(gateway.Status.Addresses) > 0 &&
			!addedGateways[gateway.Name] {
			gatewayIP := gateway.Status.Addresses[0].Value
			http2 := gateway.Name == "nativelink-gateway"
			gatewayInfo := InternalGateway{
				Name:  gateway.Name,
				IP:    gatewayIP,
				HTTP2: http2,
			}
			*gatewaysList = append(*gatewaysList, gatewayInfo)
			addedGateways[gateway.Name] = true
			requiredGateways[gateway.Name] = true
		}
	}

	for _, found := range requiredGateways {
		if !found {
			return false
		}
	}

	return true
}

func buildKubeConfig() (*rest.Config, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return nil, fmt.Errorf(
			"failed to create loadbalancer container: %w",
			err,
		)
	}

	defaultKubeconfig := filepath.Join(home, ".kube", "config")

	kubeconfig := flag.String(
		"kubeconfig",
		defaultKubeconfig,
		"path to the kubeconfig file",
	)

	flag.Parse()

	config, err := clientcmd.BuildConfigFromFlags("", *kubeconfig)
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	return config, nil
}

func writeEnvoyConfigToFile(populatedConfig string) (string, error) {
	// Create a temporary directory.
	tmpDir, err := os.MkdirTemp("", "loadbalancer")
	if err != nil {
		return "", fmt.Errorf("%w: %w", errPulumi, err)
	}

	tmpFile, err := os.CreateTemp(tmpDir, "envoy.yaml")
	if err != nil {
		return "", fmt.Errorf("failed to create temporary file: %w", err)
	}

	// Ensure the file is closed properly.
	defer tmpFile.Close()

	// Write the populated configuration to the temporary file.
	if _, err := tmpFile.WriteString(populatedConfig); err != nil {
		return "", fmt.Errorf("failed to write to temporary file: %w", err)
	}

	// Change the file mode to 0666.
	if err := os.Chmod(tmpFile.Name(), 0o666); err != nil { //nolint:mnd
		return "", fmt.Errorf("failed to set file mode: %w", err)
	}

	return tmpFile.Name(), nil
}

func createDockerContainer(
	ctx *pulumi.Context,
	name string,
	imageName string,
	absolutePath string,
	ports docker.ContainerPortArray,
	dependencies []pulumi.Resource,
) (*docker.Container, error) {
	container, err := docker.NewContainer(
		ctx,
		name,
		&docker.ContainerArgs{
			Image: pulumi.String(imageName),
			Ports: ports,
			NetworksAdvanced: docker.ContainerNetworksAdvancedArray{
				&docker.ContainerNetworksAdvancedArgs{
					Name: pulumi.String("kind"),
				},
			},
			Restart: pulumi.String("always"),
			Name:    pulumi.String(name),
			Mounts: docker.ContainerMountArray{
				&docker.ContainerMountArgs{
					Source: pulumi.String(absolutePath),
					Target: pulumi.String("/etc/envoy/envoy.yaml"),
					Type:   pulumi.String("bind"),
				},
			},
		}, pulumi.DependsOn(dependencies),
	)
	if err != nil {
		return nil, fmt.Errorf(
			"failed to create loadbalancer container: %w",
			err,
		)
	}

	return container, nil
}

func dockerContainerPorts(gateways []Gateway) docker.ContainerPortArray {
	var dockerPorts docker.ContainerPortArray
	for _, gateway := range gateways {
		dockerPorts = append(dockerPorts, &docker.ContainerPortArgs{
			Internal: pulumi.Int(gateway.InternalPort),
			External: pulumi.Int(gateway.ExternalPort),
		})
	}

	return dockerPorts
}

// Install installs a local loadbalancer on docker and makes
// it available to the kind cluster.
func (component *Loadbalancer) Install(
	ctx *pulumi.Context,
	name string,
) ([]pulumi.Resource, error) {
	config, err := buildKubeConfig()
	if err != nil {
		return nil, err
	}

	gatewayClientset, err := gatewayClient.NewForConfig(config)
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	populatedConfig, err := populateEnvoyConfig(pollGateways(
		ctx.Context(),
		gatewayClientset,
		map[string]bool{
			"nativelink-gateway": false,
			"el-gateway":         false,
			"hubble-gateway":     false,
			"tkn-gateway":        false,
			"capacitor-gateway":  false,
		},
	), component.Gateways)
	if err != nil {
		return nil, err
	}

	log.Printf(
		"Starting Envoy with the following config:\n%s",
		populatedConfig,
	)

	absolutePath, err := writeEnvoyConfigToFile(populatedConfig)
	if err != nil {
		return nil, err
	}

	loadbalancer, err := createDockerContainer(
		ctx,
		name,
		"envoyproxy/envoy:v1.19.1",
		absolutePath,
		dockerContainerPorts(component.Gateways),
		component.Dependencies,
	)
	if err != nil {
		return nil, err
	}

	return []pulumi.Resource{loadbalancer}, nil
}
