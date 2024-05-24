package components

import (
	"bytes"
	"context"
	_ "embed"
	"flag"
	"fmt"
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

// Embed the envoy_template.yaml
//
//go:embed embedded/envoy_template.yaml
var envoyTemplate string

type Port struct {
	ExternalPort int
	InternalPort int
}

// A local loadbalancer.
type Loadbalancer struct {
	Ports        []Port
	Dependencies []pulumi.Resource
}

// Gateway struct to hold name and IP.
type Gateway struct {
	Name string
	IP   string
}

// GatewaysData struct to hold multiple Gateways for templating.
type GatewaysData struct {
	Gateways []Gateway
	Ports    []Port
}

// Populate the Envoy template with gateway data.
func populateEnvoyConfig(gateways []Gateway, ports []Port) (string, error) {
	tmpl, err := template.New("envoyConfig").Parse(envoyTemplate)
	if err != nil {
		return "", fmt.Errorf("failed to parse Envoy template: %w", err)
	}

	gatewaysData := GatewaysData{
		Gateways: gateways,
		Ports:    ports,
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
) []Gateway {
	var gatewaysList []Gateway

	var mutex sync.Mutex

	var waitGroup sync.WaitGroup

	// Map to track added gateways
	addedGateways := make(map[string]bool)

	for name := range requiredGateways {
		waitGroup.Add(1)
		// Assign the function to a variable and then invoke it in the go statement
		checkAndAddGateway := func(name string) {
			defer waitGroup.Done()

			for {
				select {
				case <-ctx.Done():
					return
				default:
					if addGatewayIfNeeded(
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
		}
		go checkAndAddGateway(name)
	}

	waitGroup.Wait()

	return gatewaysList
}

func addGatewayIfNeeded(
	ctx context.Context,
	gatewayClientset *gatewayClient.Clientset,
	name string,
	addedGateways map[string]bool,
	requiredGateways map[string]bool,
	gatewaysList *[]Gateway,
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
			gatewayInfo := Gateway{
				Name: gateway.Name,
				IP:   gatewayIP,
			}
			*gatewaysList = append(*gatewaysList, gatewayInfo)
			addedGateways[gateway.Name] = true
			requiredGateways[gateway.Name] = true
		}
	}

	return allGatewaysFound(requiredGateways)
}

func allGatewaysFound(requiredGateways map[string]bool) bool {
	for _, found := range requiredGateways {
		if !found {
			return false
		}
	}

	return true
}

func getHomeDir() (string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", fmt.Errorf("%w: %w", errPulumi, err)
	}

	return home, nil
}

func buildKubeConfig(kubeconfig *string) (*rest.Config, error) {
	config, err := clientcmd.BuildConfigFromFlags("", *kubeconfig)
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	return config, nil
}

func createGatewayClientset(
	config *rest.Config,
) (*gatewayClient.Clientset, error) {
	gatewayClientset, err := gatewayClient.NewForConfig(config)
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	return gatewayClientset, nil
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
	if _, err := tmpFile.Write([]byte(populatedConfig)); err != nil {
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

func parseFlags(defaultKubeconfig string) *string {
	kubeconfig := flag.String(
		"kubeconfig",
		defaultKubeconfig,
		"path to the kubeconfig file",
	)

	flag.Parse()

	return kubeconfig
}

func prepareDockerContainerPorts(ports []Port) docker.ContainerPortArray {
	var dockerPorts docker.ContainerPortArray
	for _, port := range ports {
		dockerPorts = append(dockerPorts, &docker.ContainerPortArgs{
			Internal: pulumi.Int(port.InternalPort),
			External: pulumi.Int(port.ExternalPort),
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
	home, err := getHomeDir()
	if err != nil {
		return nil, err
	}

	defaultKubeconfig := filepath.Join(home, ".kube", "config")
	kubeconfig := parseFlags(defaultKubeconfig)

	config, err := buildKubeConfig(kubeconfig)
	if err != nil {
		return nil, err
	}

	gatewayClientset, err := createGatewayClientset(config)
	if err != nil {
		return nil, err
	}

	requiredGateways := map[string]bool{
		"scheduler-gateway": false,
		"cache-gateway":     false,
		"el-gateway":        false,
		"hubble-gateway":    false,
		"tkn-gateway":       false,
	}

	gatewaysList := pollGateways(
		ctx.Context(),
		gatewayClientset,
		requiredGateways,
	)

	populatedConfig, err := populateEnvoyConfig(gatewaysList, component.Ports)
	if err != nil {
		return nil, err
	}

	absolutePath, err := writeEnvoyConfigToFile(populatedConfig)
	if err != nil {
		return nil, err
	}

	ports := prepareDockerContainerPorts(component.Ports)

	loadbalancer, err := createDockerContainer(
		ctx,
		name,
		"envoyproxy/envoy:v1.19.1",
		absolutePath,
		ports,
		component.Dependencies,
	)
	if err != nil {
		return nil, err
	}

	return []pulumi.Resource{loadbalancer}, nil
}
