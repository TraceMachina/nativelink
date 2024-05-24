package clusters

import (
	"bytes"
	"context"
	_ "embed"
	"errors"
	"fmt"
	"log"
	"runtime"

	"github.com/docker/docker/api/types"
	"github.com/docker/docker/client"
	git "github.com/go-git/go-git/v5"
	"gopkg.in/yaml.v3"
	"sigs.k8s.io/kind/pkg/cluster"
)

var errKind = errors.New("kind error")

// gitSrcRoot returns the absolute path to the root of the git repository where
// this function is invoked from.
func gitSrcRoot() string {
	repo, err := git.PlainOpenWithOptions(
		".",
		&git.PlainOpenOptions{DetectDotGit: true},
	)
	if err != nil {
		log.Fatalf("Failed to open git repository: %v", err)
	}

	worktree, err := repo.Worktree()
	if err != nil {
		log.Fatalf("Failed to get worktree: %v", err)
	}

	return worktree.Filesystem.Root()
}

// CreateLocalKindConfig creates a kind configuration with several tweaks for
// local development.
//
//  1. The Git repository where the cluster is created from is mounted at
//     `/mnt/src_root` so that jobs running inside the cluster always use the
//     latest sources which may have been modified locally.
//  2. The host's `/nix` store is mounted as readonly into the cluster nodes at
//     `/nix`. This allows creating `PersistentVolumes` that make the hosts nix
//     store available to e.g. pipelines running in the cluster. This way such
//     pipelines don't need to build rebuild anything that was built on the host
//     already.
//  3. Nodes are set up to be compatible with CubeFS. By default these configs
//     are unused. However, if CubeFS is deployed to the cluster, the data in
//     these volumes persists beyond cluster destruction. Combined with CubeFS's
//     CSI driver this allows creating realistic PersistentVolumes on the fly and
//     in combination with the S3 endpoints allows simulating S3 storage buckets.
//  4. Containerd in the nodes is patched to be compatible with a pass-through
//     container registry. This allows creating a registry that attaches to both
//     the hosts default "bridge" network and the cluster's "kind" network so
//     that images copied to the host's local registry become available to the
//     kind nodes as well.

//go:embed config.yaml
var kindDevConfig string

type ExtraMount struct {
	HostPath      string `yaml:"hostPath"`
	ContainerPath string `yaml:"containerPath"`
	ReadOnly      bool   `yaml:"readOnly,omitempty"`
}

type Networking struct {
	DisableDefaultCNI bool   `yaml:"disableDefaultCNI"` //nolint:tagliatelle
	KubeProxyMode     string `yaml:"kubeProxyMode"`
}

type Node struct {
	Role        string       `yaml:"role"`
	ExtraMounts []ExtraMount `yaml:"extraMounts,omitempty"`
}

type ClusterConfig struct {
	Kind                    string     `yaml:"kind"`
	APIVersion              string     `yaml:"apiVersion"`
	Networking              Networking `yaml:"networking"`
	Nodes                   []Node     `yaml:"nodes"`
	ContainerdConfigPatches []string   `yaml:"containerdConfigPatches"`
}

func (c *ClusterConfig) get() *ClusterConfig {
	err := yaml.Unmarshal([]byte(kindDevConfig), c)
	if err != nil {
		log.Fatalf("Unmarshal: %v", err)
	}

	return c
}

func CreateLocalKindConfig() bytes.Buffer {
	var config ClusterConfig

	config.get()

	gitRoot := gitSrcRoot()

	for node := range config.Nodes {
		if config.Nodes[node].Role == "worker" {
			config.Nodes[node].ExtraMounts = append(
				config.Nodes[node].ExtraMounts,
				ExtraMount{
					HostPath:      gitRoot,
					ContainerPath: "/mnt/src_root",
				},
			)
			if runtime.GOOS == "linux" {
				config.Nodes[node].ExtraMounts = append(
					config.Nodes[node].ExtraMounts,
					ExtraMount{
						HostPath:      "/nix",
						ContainerPath: "/nix",
						ReadOnly:      false,
					},
				)
			}
		}
	}

	// Marshal the config back to YAML.
	configYAML, err := yaml.Marshal(config)
	if err != nil {
		log.Fatalf("Error marshalling config to YAML: %v", err)
	}

	var kindConfig bytes.Buffer

	kindConfig.Write(configYAML)

	return kindConfig
}

// CreateLocalCluster creates a local kind cluster with the config from
// CreateLocalKindConfig.
func CreateLocalCluster(
	provider *cluster.Provider,
	internalRegistryPort int,
	externalRegistryPort int,
) error {
	log.Printf("Creating kind cluster.")

	kindConfig := CreateLocalKindConfig()

	log.Println("Instantiating Kind Cluster with the following config:")
	log.Print(kindConfig.String())

	if err := provider.Create(
		"kind",
		cluster.CreateWithRawConfig(kindConfig.Bytes()),
	); err != nil {
		return fmt.Errorf("%w: %w", errKind, err)
	}

	if err := configureLocalRegistry("kind-registry", internalRegistryPort, externalRegistryPort); err != nil {
		return fmt.Errorf("%w: %w", errKind, err)
	}

	return nil
}

// DeleteLocalCluster removes a local kind cluster.
func DeleteLocalCluster(provider *cluster.Provider) error {
	log.Printf("Deleting cluster...")

	if err := provider.Delete("kind", ""); err != nil {
		return fmt.Errorf("%w: %w", errKind, err)
	}

	return nil
}

// configureLocalRegistry adjusts all nodes in a kind cluster nodes to be
// compatible with a local passthrough registry. This function configures the
// nodes but doesn't start an actual registry.
func configureLocalRegistry(
	registryName string,
	internalPort int,
	externalPort int,
) error {
	cli, err := client.NewClientWithOpts(
		client.FromEnv,
		client.WithAPIVersionNegotiation(),
	)
	if err != nil {
		return fmt.Errorf("error creating Docker client: %w", err)
	}

	ctx := context.Background()

	nodeNames := []string{
		"kind-control-plane",
		"kind-worker",
		"kind-worker2",
		"kind-worker3",
	}

	for _, nodeName := range nodeNames {
		if err := createRegistryConfigInNode(ctx, cli, nodeName, registryName, internalPort, externalPort); err != nil {
			return fmt.Errorf(
				"error configuring kind node %s: %w",
				nodeName,
				err,
			)
		}
	}

	return nil
}

// createRegistryConfigInNode configures a single kind node to be compatible
// with a local passthrough registry. This function configures the node but
// doesn't start any actual registry.
func createRegistryConfigInNode(
	ctx context.Context,
	cli *client.Client,
	nodeName string, regName string, internalPort int, externalPort int,
) error {
	config := fmt.Sprintf("[host.\"http://%s:%d\"]", regName, internalPort)
	regDir := fmt.Sprintf("/etc/containerd/certs.d/localhost:%d", externalPort)
	execConfig := types.ExecConfig{
		Cmd: []string{
			"sh",
			"-c",
			fmt.Sprintf(
				"mkdir -p %s && echo '%s' > %s/hosts.toml",
				regDir,
				config,
				regDir,
			),
		},
	}

	execID, err := cli.ContainerExecCreate(ctx, nodeName, execConfig)
	if err != nil {
		return fmt.Errorf(
			"error creating exec instance for node %s: %w",
			nodeName,
			err,
		)
	}

	if err := cli.ContainerExecStart(ctx, execID.ID, types.ExecStartCheck{}); err != nil {
		return fmt.Errorf(
			"error starting exec command on node %s: %w",
			nodeName,
			err,
		)
	}

	return nil
}
