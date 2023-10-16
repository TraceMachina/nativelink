resource "google_compute_image" "tfer--base-image-2" {
  disk_size_gb = "10"

  guest_os_features {
    type = "GVNIC"
  }

  guest_os_features {
    type = "SEV_CAPABLE"
  }

  guest_os_features {
    type = "SEV_LIVE_MIGRATABLE"
  }

  guest_os_features {
    type = "SEV_SNP_CAPABLE"
  }

  guest_os_features {
    type = "UEFI_COMPATIBLE"
  }

  guest_os_features {
    type = "VIRTIO_SCSI_MULTIQUEUE"
  }

  licenses    = ["https://www.googleapis.com/compute/v1/projects/ubuntu-os-cloud/global/licenses/ubuntu-2204-lts"]
  name        = "base-image-2"
  project     = "turbo-cache-gcp-test"
  source_disk = "https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/zones/us-central1-c/disks/instance-1"
}
