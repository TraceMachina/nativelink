# Copyright 2023 The NativeLink Authors. All rights reserved.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#    http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

resource "google_compute_region_autoscaler" "x86_cpu_worker_autoscaler" {
  name     = "${var.project_prefix}-x86-cpu-worker-autoscaler"
  provider = google-beta
  project  = var.gcp_project_id
  region   = var.gcp_region

  autoscaling_policy {
    max_replicas    = 10
    min_replicas    = 1
    cooldown_period = 60 # 1 minutes.

    metric {
      name   = "custom.googleapis.com/nativelink/x86_workers/cpu_ratio"
      target = 1.0
      type   = "GAUGE"
      filter = "resource.type=\"global\""
    }

    scale_in_control {
      max_scaled_in_replicas {
        fixed = 10
      }
      time_window_sec = 120 # 2 minutes.
    }
  }

  target = google_compute_region_instance_group_manager.x86_cpu_worker_instance_group.id
}

resource "google_compute_region_instance_group_manager" "x86_cpu_worker_instance_group" {
  base_instance_name = "${var.project_prefix}-x86-cpu-worker-group"
  name               = "${var.project_prefix}-x86-cpu-worker-instance-group"

  version {
    instance_template = google_compute_region_instance_template.x86_cpu_worker_instance_template.id
  }

  auto_healing_policies {
    health_check      = google_compute_region_health_check.x86_cpu_worker_health_checker.id
    initial_delay_sec = 120 # 2 minutes.
  }

  wait_for_instances_status = "STABLE"
}

resource "google_compute_region_instance_template" "x86_cpu_worker_instance_template" {
  name = "${var.project_prefix}-x86-cpu-worker-instance-template"

  machine_type   = var.x86_cpu_worker_machine_type
  can_ip_forward = false

  scheduling {
    instance_termination_action = "STOP"
    automatic_restart           = false
    provisioning_model          = "SPOT"
    preemptible                 = true
  }

  service_account {
    email  = google_service_account.x86_cpu_worker_service_account.email
    scopes = ["cloud-platform"]
  }

  disk {
    source_image = google_compute_image.base_image.id
    auto_delete  = true
    boot         = true
  }

  disk {
    disk_type    = "local-ssd"
    type         = "SCRATCH"
    disk_size_gb = "375"
  }

  network_interface {
    network = data.google_compute_network.default.id

    # TODO(allada) We need to give it a public ip to access google services
    # like bucket and secrets manager. Google does support private access to
    # these services, but it requires a lot of configurations.
    access_config {
      # Ephemeral.
      network_tier = "STANDARD"
    }
  }

  metadata = {
    nativelink-type                               = "worker"
    nativelink-internal-worker-scheduler-endpoint = trim(google_dns_record_set.scheduler_internal_for_worker_dns_record_set.name, ".")
    nativelink-cas-bucket                         = "${google_storage_bucket.cas_s3_bucket.name}:${google_storage_bucket.cas_s3_bucket.location}"
    nativelink-ac-bucket                          = "${google_storage_bucket.ac_s3_bucket.name}:${google_storage_bucket.ac_s3_bucket.location}"
    nativelink-hmac-secret-key                    = google_secret_manager_secret.x86_cpu_worker_secret_manager_hmac_key.secret_id
    nativelink-browser-endpoint                   = trim(google_dns_record_set.browser_dns_record_set.name, ".")
  }
}

resource "google_compute_region_health_check" "x86_cpu_worker_health_checker" {
  name               = "${var.project_prefix}-x86-cpu-worker-health-checker"
  check_interval_sec = "5"
  healthy_threshold  = "2"

  http_health_check {
    port         = "50051"
    request_path = "/status"
    response     = "Ok"
  }

  log_config {
    enable = "false"
  }

  timeout_sec         = "5"
  unhealthy_threshold = "2"
}

resource "google_service_account" "x86_cpu_worker_service_account" {
  account_id = "${var.project_prefix}-x86-worker-sa"
}

resource "google_project_iam_member" "x86_cpu_worker_secret_manager_iam_member" {
  member  = "serviceAccount:${google_service_account.x86_cpu_worker_service_account.email}"
  project = var.gcp_project_id
  role    = google_project_iam_custom_role.secret_manager_access_role.name
}

resource "google_storage_hmac_key" "x86_cpu_worker_hmac_key" {
  service_account_email = google_service_account.x86_cpu_worker_service_account.email
}

resource "google_secret_manager_secret" "x86_cpu_worker_secret_manager_hmac_key" {
  secret_id = "${var.project_prefix}-x86-cpu-worker-secret-manager-hmac-key"

  replication {
    auto {}
  }

  depends_on = [google_project_service.secret_manager_api]
}

# TODO(allada) Setup hmac key rotation using a lambda or similar.
resource "google_secret_manager_secret_version" "x86_cpu_worker_secret_manager_version_hmac_key" {
  secret          = google_secret_manager_secret.x86_cpu_worker_secret_manager_hmac_key.id
  secret_data     = "${google_storage_hmac_key.x86_cpu_worker_hmac_key.access_id}:${google_storage_hmac_key.x86_cpu_worker_hmac_key.secret}"
  deletion_policy = "DELETE"
}

resource "google_secret_manager_secret_iam_member" "x86_cpu_worker_secret_accessor_iam_member" {
  project   = google_secret_manager_secret.x86_cpu_worker_secret_manager_hmac_key.project
  secret_id = google_secret_manager_secret.x86_cpu_worker_secret_manager_hmac_key.secret_id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.x86_cpu_worker_service_account.email}"
}

resource "google_storage_bucket_iam_member" "x86_cpu_worker_service_account_cas_bucket_iam_member" {
  bucket = google_storage_bucket.cas_s3_bucket.name
  role   = "roles/storage.objectUser"
  member = "serviceAccount:${google_service_account.x86_cpu_worker_service_account.email}"
}

resource "google_storage_bucket_iam_member" "x86_cpu_worker_service_account_ac_bucket_iam_member" {
  bucket = google_storage_bucket.ac_s3_bucket.name
  role   = "roles/storage.objectUser"
  member = "serviceAccount:${google_service_account.x86_cpu_worker_service_account.email}"
}
