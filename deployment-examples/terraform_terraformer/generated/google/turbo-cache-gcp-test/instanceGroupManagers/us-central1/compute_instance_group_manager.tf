resource "google_compute_instance_group_manager" "tfer--turbo-cache-cas-instance-group" {
  base_instance_name = "turbo-cache-cas-instance-group"
  name               = "turbo-cache-cas-instance-group"

  named_port {
    name = "cas"
    port = "50051"
  }

  project     = "turbo-cache-gcp-test"
  target_size = "0"

  update_policy {
    max_surge_fixed         = "1"
    max_surge_percent       = "0"
    max_unavailable_fixed   = "1"
    max_unavailable_percent = "0"
    minimal_action          = "REPLACE"
    replacement_method      = "SUBSTITUTE"
    type                    = "OPPORTUNISTIC"
  }

  version {
    instance_template = "https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/regions/us-central1/instanceTemplates/turbo-cache-cas-instance-template-3"
  }

  wait_for_instances_status = "STABLE"
  zone                      = "us-central1-c"
}

resource "google_compute_instance_group_manager" "tfer--turbo-cache-scheduler-instance-group" {
  auto_healing_policies {
    health_check      = "https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/global/healthChecks/turbo-cache-scheduler-health-checker"
    initial_delay_sec = "300"
  }

  base_instance_name = "turbo-cache-scheduler-instance-group"
  name               = "turbo-cache-scheduler-instance-group"

  named_port {
    name = "scheduler"
    port = "50052"
  }

  project     = "turbo-cache-gcp-test"
  target_size = "0"

  update_policy {
    max_surge_fixed         = "1"
    max_surge_percent       = "0"
    max_unavailable_fixed   = "1"
    max_unavailable_percent = "0"
    minimal_action          = "REPLACE"
    replacement_method      = "SUBSTITUTE"
    type                    = "OPPORTUNISTIC"
  }

  version {
    instance_template = "https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/regions/us-central1/instanceTemplates/turbo-cache-scheduler-instance-template-1"
  }

  wait_for_instances_status = "STABLE"
  zone                      = "us-central1-c"
}

resource "google_compute_instance_group_manager" "tfer--turbo-cache-x86-cpu-worker-instance-group" {
  base_instance_name = "turbo-cache-x86-cpu-worker-instance-group"
  name               = "turbo-cache-x86-cpu-worker-instance-group"
  project            = "turbo-cache-gcp-test"
  target_size        = "0"

  update_policy {
    max_surge_fixed         = "1"
    max_surge_percent       = "0"
    max_unavailable_fixed   = "1"
    max_unavailable_percent = "0"
    minimal_action          = "REPLACE"
    replacement_method      = "SUBSTITUTE"
    type                    = "OPPORTUNISTIC"
  }

  version {
    instance_template = "https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/regions/us-central1/instanceTemplates/turbo-cache-x86-cpu-worker-instance-template-3"
  }

  wait_for_instances_status = "STABLE"
  zone                      = "us-central1-c"
}
