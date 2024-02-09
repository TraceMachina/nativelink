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

resource "google_compute_region_autoscaler" "cas_autoscaler" {
  name     = "${var.project_prefix}-cas-autoscaler"
  provider = google-beta
  project  = var.gcp_project_id
  region   = var.gcp_region

  autoscaling_policy {
    max_replicas    = 10
    min_replicas    = 1
    cooldown_period = 60 # 1 minutes.

    load_balancing_utilization {
      target = 1
    }

    scale_in_control {
      max_scaled_in_replicas {
        fixed = 10
      }
      time_window_sec = 120 # 2 minutes.
    }
  }

  target = google_compute_region_instance_group_manager.cas_instance_group.id
}

resource "google_compute_region_instance_group_manager" "cas_instance_group" {
  base_instance_name = "${var.project_prefix}-cas-group"
  name               = "${var.project_prefix}-cas-instance-group"
  region             = var.gcp_region

  named_port {
    name = "cas"
    port = "50051"
  }

  target_size = "1"

  version {
    instance_template = google_compute_region_instance_template.cas_instance_template.id
  }

  auto_healing_policies {
    health_check      = google_compute_region_health_check.cas_region_health_checker.id
    initial_delay_sec = 300
  }

  wait_for_instances_status = "STABLE"
}

resource "google_compute_region_instance_template" "cas_instance_template" {
  name = "${var.project_prefix}-cas-instance-template"

  # Use a very small instance type for the CAS, since it's just a proxy to S3.
  machine_type   = var.cas_machine_type
  can_ip_forward = false

  service_account {
    email  = google_service_account.cas_service_account.email
    scopes = ["cloud-platform"]
  }

  scheduling {
    automatic_restart   = true
    on_host_maintenance = "MIGRATE"
  }

  disk {
    source_image = google_compute_image.base_image.id
    auto_delete  = true
    boot         = true
  }

  network_interface {
    network = data.google_compute_network.default.id

    # Give it a public IP.
    access_config {
      # Ephemeral.
      network_tier = "STANDARD"
    }
  }

  metadata = {
    nativelink-type            = "cas"
    nativelink-cas-bucket      = "${google_storage_bucket.cas_s3_bucket.name}:${google_storage_bucket.cas_s3_bucket.location}"
    nativelink-ac-bucket       = "${google_storage_bucket.ac_s3_bucket.name}:${google_storage_bucket.ac_s3_bucket.location}"
    nativelink-hmac-secret-key = google_secret_manager_secret.cas_secret_manager_hmac_key.secret_id
  }
}

resource "google_compute_region_health_check" "cas_region_health_checker" {
  name               = "${var.project_prefix}-cas-region-health-checker"
  check_interval_sec = "15"
  healthy_threshold  = "2"

  http2_health_check {
    port         = "50051"
    request_path = "/status"
    response     = "Ok"
  }

  log_config {
    enable = "false"
  }

  timeout_sec         = "5"
  unhealthy_threshold = "3"
}

resource "google_compute_health_check" "cas_health_checker" {
  name               = "${var.project_prefix}-cas-health-checker"
  check_interval_sec = "15"
  healthy_threshold  = "2"

  http2_health_check {
    port         = "50051"
    request_path = "/status"
    response     = "Ok"
  }

  log_config {
    enable = "false"
  }

  timeout_sec         = "5"
  unhealthy_threshold = "3"
}

resource "google_compute_backend_service" "cas_backend_service" {
  name                    = "${var.project_prefix}-cas-backend-service"
  affinity_cookie_ttl_sec = "0"

  backend {
    balancing_mode  = "UTILIZATION"
    capacity_scaler = "1"
    group           = google_compute_region_instance_group_manager.cas_instance_group.instance_group
    max_utilization = "0.8"
  }

  connection_draining_timeout_sec = "300"
  enable_cdn                      = "false"
  health_checks = [
    google_compute_health_check.cas_health_checker.id,
  ]
  load_balancing_scheme = "EXTERNAL_MANAGED"
  locality_lb_policy    = "ROUND_ROBIN"

  log_config {
    enable = "false"
  }

  port_name        = "cas"
  protocol         = "HTTP2"
  session_affinity = "NONE"
  timeout_sec      = "30"
}

resource "google_compute_url_map" "cas_url_map" {
  name            = "${var.project_prefix}-cas-url-map"
  default_service = google_compute_backend_service.cas_backend_service.id
}

resource "google_compute_target_https_proxy" "cas_http_proxy" {
  name            = "${var.project_prefix}-cas-http-proxy"
  url_map         = google_compute_url_map.cas_url_map.id
  certificate_map = "//certificatemanager.googleapis.com/${data.google_certificate_manager_certificate_map.default.id}"
  proxy_bind      = "false"
  quic_override   = "NONE"
  ssl_policy      = google_compute_ssl_policy.lb_ssl_policy.id
}

resource "google_compute_global_forwarding_rule" "cas_forwarding_rule" {
  name                  = "${var.project_prefix}-cas-forwarding-rule"
  ip_protocol           = "TCP"
  ip_version            = "IPV4"
  load_balancing_scheme = "EXTERNAL_MANAGED"
  port_range            = "443-443"
  target                = google_compute_target_https_proxy.cas_http_proxy.id
}

resource "google_dns_record_set" "cas_dns_record_set" {
  name         = "cas.${data.google_dns_managed_zone.dns_zone.dns_name}"
  type         = "A"
  ttl          = 300
  managed_zone = data.google_dns_managed_zone.dns_zone.name

  rrdatas = [
    google_compute_global_forwarding_rule.cas_forwarding_rule.ip_address
  ]
}

resource "google_service_account" "cas_service_account" {
  account_id = "${var.project_prefix}-cas-sa"
}

resource "google_storage_hmac_key" "cas_hmac_key" {
  service_account_email = google_service_account.cas_service_account.email
}

resource "google_secret_manager_secret" "cas_secret_manager_hmac_key" {
  secret_id = "${var.project_prefix}-cas-secret-manager-hmac-key"

  replication {
    auto {}
  }

  depends_on = [google_project_service.secret_manager_api]
}

# TODO(allada) Setup hmac key rotation using a lambda or similar.
resource "google_secret_manager_secret_version" "cas_secret_manager_version_hmac_key" {
  secret          = google_secret_manager_secret.cas_secret_manager_hmac_key.id
  secret_data     = "${google_storage_hmac_key.cas_hmac_key.access_id}:${google_storage_hmac_key.cas_hmac_key.secret}"
  deletion_policy = "DELETE"
}

resource "google_secret_manager_secret_iam_member" "cas_secret_accessor_iam_member" {
  project   = google_secret_manager_secret.cas_secret_manager_hmac_key.project
  secret_id = google_secret_manager_secret.cas_secret_manager_hmac_key.secret_id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.cas_service_account.email}"
}

resource "google_storage_bucket_iam_member" "cas_service_account_cas_bucket_iam_member" {
  bucket = google_storage_bucket.cas_s3_bucket.name
  role   = "roles/storage.objectUser"
  member = "serviceAccount:${google_service_account.cas_service_account.email}"
}

resource "google_storage_bucket_iam_member" "cas_service_account_ac_bucket_iam_member" {
  bucket = google_storage_bucket.ac_s3_bucket.name
  role   = "roles/storage.objectUser"
  member = "serviceAccount:${google_service_account.cas_service_account.email}"
}
