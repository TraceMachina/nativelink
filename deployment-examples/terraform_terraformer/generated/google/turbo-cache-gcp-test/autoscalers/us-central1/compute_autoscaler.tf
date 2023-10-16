resource "google_compute_autoscaler" "tfer--us-central1-c-002F-turbo-cache-cas-instance-group" {
  autoscaling_policy {
    cooldown_period = "60"

    cpu_utilization {
      predictive_method = "NONE"
      target            = "0.6"
    }

    max_replicas = "10"
    min_replicas = "1"
    mode         = "OFF"
  }

  name    = "turbo-cache-cas-instance-group"
  project = "turbo-cache-gcp-test"
  target  = "https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/zones/us-central1-c/instanceGroupManagers/turbo-cache-cas-instance-group"
  zone    = "https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/zones/us-central1-c"
}

resource "google_compute_autoscaler" "tfer--us-central1-c-002F-turbo-cache-scheduler-instance-group" {
  autoscaling_policy {
    cooldown_period = "60"

    cpu_utilization {
      predictive_method = "NONE"
      target            = "0.6"
    }

    max_replicas = "10"
    min_replicas = "0"
    mode         = "OFF"
  }

  name    = "turbo-cache-scheduler-instance-group"
  project = "turbo-cache-gcp-test"
  target  = "https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/zones/us-central1-c/instanceGroupManagers/turbo-cache-scheduler-instance-group"
  zone    = "https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/zones/us-central1-c"
}

resource "google_compute_autoscaler" "tfer--us-central1-c-002F-turbo-cache-x86-cpu-worker-instance-group" {
  autoscaling_policy {
    cooldown_period = "60"

    cpu_utilization {
      predictive_method = "NONE"
      target            = "0.6"
    }

    max_replicas = "10"
    min_replicas = "1"
    mode         = "OFF"
  }

  name    = "turbo-cache-x86-cpu-worker-instance-group"
  project = "turbo-cache-gcp-test"
  target  = "https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/zones/us-central1-c/instanceGroupManagers/turbo-cache-x86-cpu-worker-instance-group"
  zone    = "https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/zones/us-central1-c"
}
