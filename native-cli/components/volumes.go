package components

import (
	"fmt"

	corev1 "github.com/pulumi/pulumi-kubernetes/sdk/v4/go/kubernetes/core/v1"
	metav1 "github.com/pulumi/pulumi-kubernetes/sdk/v4/go/kubernetes/meta/v1"
	"github.com/pulumi/pulumi/sdk/v3/go/pulumi"
)

// Configuration to create a PV which mount a node-local directory via
// `hostPath`.
type LocalPV struct {
	Size     string
	HostPath string
}

// Install creates a PersistentVolume from a `LocalPV` config.
func (component *LocalPV) Install(
	ctx *pulumi.Context,
	name string,
) ([]pulumi.Resource, error) {
	pvName := name + "-pv"

	persistentVolume, err := corev1.NewPersistentVolume(
		ctx,
		pvName,
		&corev1.PersistentVolumeArgs{
			ApiVersion: pulumi.String("v1"),
			Kind:       pulumi.String("PersistentVolume"),
			Metadata: &metav1.ObjectMetaArgs{
				Name:   pulumi.String(pvName),
				Labels: pulumi.StringMap{"type": pulumi.String("local")},
			},
			Spec: &corev1.PersistentVolumeSpecArgs{
				StorageClassName: pulumi.String("manual"),
				Capacity: pulumi.StringMap{
					"storage": pulumi.String(component.Size),
				},
				AccessModes: pulumi.StringArray{pulumi.String("ReadWriteMany")},
				HostPath: &corev1.HostPathVolumeSourceArgs{
					Path: pulumi.String(component.HostPath),
				},
			},
		},
	)
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

	return []pulumi.Resource{persistentVolume}, nil
}
