resource "google_compute_instance_group" "tfer--us-central1-c-002F-turbo-cache-cas-instance-group" {
  description = "This instance group is controlled by Instance Group Manager 'turbo-cache-cas-instance-group'. To modify instances in this group, use the Instance Group Manager API: https://cloud.google.com/compute/docs/reference/latest/instanceGroupManagers"
  name        = "turbo-cache-cas-instance-group"

  named_port {
    name = "cas"
    port = "50051"
  }

  network = "https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/global/networks/default"
  project = "turbo-cache-gcp-test"
  zone    = "us-central1-c"
}

resource "google_compute_instance_group" "tfer--us-central1-c-002F-turbo-cache-scheduler-instance-group" {
  description = "This instance group is controlled by Instance Group Manager 'turbo-cache-scheduler-instance-group'. To modify instances in this group, use the Instance Group Manager API: https://cloud.google.com/compute/docs/reference/latest/instanceGroupManagers"
  name        = "turbo-cache-scheduler-instance-group"

  named_port {
    name = "scheduler"
    port = "50052"
  }

  network = "https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/global/networks/default"
  project = "turbo-cache-gcp-test"
  zone    = "us-central1-c"
}

resource "google_compute_instance_group" "tfer--us-central1-c-002F-turbo-cache-x86-cpu-worker-instance-group" {
  description = "This instance group is controlled by Instance Group Manager 'turbo-cache-x86-cpu-worker-instance-group'. To modify instances in this group, use the Instance Group Manager API: https://cloud.google.com/compute/docs/reference/latest/instanceGroupManagers"
  name        = "turbo-cache-x86-cpu-worker-instance-group"
  network     = "https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/global/networks/default"
  project     = "turbo-cache-gcp-test"
  zone        = "us-central1-c"
}
