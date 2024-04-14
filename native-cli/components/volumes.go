package components

import (
	"fmt"

	corev1 "github.com/pulumi/pulumi-kubernetes/sdk/v4/go/kubernetes/core/v1"
	metav1 "github.com/pulumi/pulumi-kubernetes/sdk/v4/go/kubernetes/meta/v1"
	"github.com/pulumi/pulumi/sdk/v3/go/pulumi"
)

// Configuration to create a PV and PVC which mount a node-local directory via
// `hostPath`.
type LocalPVAndPVC struct {
	Size     string
	HostPath string
}

// Install installs the PV and PVC from a `LocalPVAndPVC` config.
func (component *LocalPVAndPVC) Install(
	ctx *pulumi.Context,
	name string,
) ([]pulumi.Resource, error) {
	pvName := name + "-pv"
	pvcName := name + "-pvc"

	persistentVolumeClaim, err := corev1.NewPersistentVolumeClaim(
		ctx,
		pvcName,
		&corev1.PersistentVolumeClaimArgs{
			ApiVersion: pulumi.String("v1"),
			Kind:       pulumi.String("PersistentVolumeClaim"),
			Metadata: &metav1.ObjectMetaArgs{
				Name: pulumi.String(pvcName),
			},
			Spec: &corev1.PersistentVolumeClaimSpecArgs{
				StorageClassName: pulumi.String("manual"),
				VolumeName:       pulumi.String(pvName),
				AccessModes: pulumi.StringArray{
					pulumi.String("ReadWriteMany"),
				},
				Resources: &corev1.VolumeResourceRequirementsArgs{
					Requests: pulumi.StringMap{
						"storage": pulumi.String(component.Size),
					},
				},
			},
		},
	)
	if err != nil {
		return nil, fmt.Errorf("%w: %w", errPulumi, err)
	}

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

	return []pulumi.Resource{persistentVolume, persistentVolumeClaim}, nil
}
