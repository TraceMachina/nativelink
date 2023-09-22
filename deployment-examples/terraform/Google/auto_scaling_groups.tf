# Copyright 2022 The Turbo Cache Authors. All rights reserved.
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

# --- Begin CAS ---

resource "google_compute_instance_template" "cas_instance_template" {
  name_prefix  = "turbo_cache_cas_instance_template"
  machine_type = "n1-standard-1" # change according to your needs
  
  instance_description = "instance controlled by terraform"
  automatic_restart    = true
  can_ip_forward       = false

  disk {
    source_image = "projects/debian-cloud/global/images/family/debian-11" # Adjust with your image
    auto_delete  = true
    boot         = true
  }

  network_interface {
    network = "default"
  }

  service_account {
    # specify the service account and scopes
    email  = google_service_account.cas_service_account.email
    scopes = ["cloud-platform"]
  }

  metadata = {
    turbo_cache-instance-type = "cas"
    turbo_cache-s3-cas-bucket = google_storage_bucket.cas_bucket.name
  }

  scheduling {
    preemptible       = false
    on_host_maintenance = "MIGRATE"
  }

  lifecycle {
    create_before_destroy = true
  }
}


resource "google_compute_instance_group_manager" "cas_instance_group_manager" {
  name = "turbo_cache_cas_instance_group_manager"

  base_instance_name = "cas-instance"
  update_strategy    = "NONE"

  instance_template = google_compute_instance_template.cas_instance_template.self_link

  target_size  = 1
  target_pools = [google_compute_target_pool.cas_target_pool.self_link]

  named_port {
    name = "http"
    port = 80
  }

  auto_healing_policies {
    health_check      = google_compute_health_check.auto_healing_check.self_link
    initial_delay_sec = 300
  }

  version {
    instance_template = google_compute_instance_template.cas_instance_template.self_link
  }

  autoscaler {
    max_replicas    = 25
    min_replicas    = 1
    cooldown_period = 300
    autoscaling_policy {
      scale_up_control {
        max_scaled_up_replicas = 25
        time_window_sec        = 300
      }

      scale_down_control {
        max_scaled_down_replicas = 0 
        time_window_sec          = 300
      }

      # custom_metric_utilizations {
      #  metric = "custom.cloudmonitoring.googleapis.com/(TODO marcussorealheis add metric)" 
      #  utilization_target_type = "GAUGE"
      #  utilization_target       = 60
      # }
    }
  }
}

# --- End CAS ---
# --- Begin Scheduler ---

resource "google_compute_instance_template" "scheduler_instance_template" {
  name_prefix  = "turbo_cache_scheduler_instance_template-"
  machine_type = "n1-standard-1"  # Set the appropriate machine type

  disk {
    source_image = "projects/debian-cloud/global/images/family/debian-11"  # Set the appropriate image
    auto_delete  = true
    boot         = true
  }

  network_interface {
    network = "default"  # Set the appropriate network

    # Apply a tag to allow SSH traffic
    access_config {
      nat_ip       = google_compute_address.static_ip.address
      network_tier = "STANDARD"
    }

  }

  service_account {
    email  = google_service_account.scheduler_service_account.email
    scopes = ["cloud-platform"]
  }

  metadata = {
    "turbo_cache:instance_type" = "scheduler"
    "turbo_cache:gcp_bucket"    = google_storage_bucket.cas_bucket.name
  }

  tags = ["allow-ssh"]  # Apply a tag to allow SSH traffic

  lifecycle {
    create_before_destroy = true
  }
}

resource "google_service_account" "scheduler_autoscale_service_account" {
  account_id   = "scheduler-autoscale-service-account"
  display_name = "Scheduler Autoscale Service Account"
}

resource "google_storage_bucket" "cas_bucket" {
  name     = "turbo_cache_cas_bucket"
  location = "US"
}

resource "google_compute_instance_group_manager" "scheduler_instance_group" {
  name = "turbo_cache_scheduler_instance_group"
  version {
    instance_template = google_compute_instance_template.scheduler_instance_template.self_link
  }

  base_instance_name = "scheduler-instance"
  zone               = "us-central1-a"
  target_size        = 1

  named_port {
    name = "http"
    port = 80
  }

  auto_healing_policies {
    health_check      = google_compute_health_check.auto_healing_check.name
    initial_delay_sec = 300
  }

  lifecycle {
    create_before_destroy = true
  }
}

resource "google_compute_health_check" "auto_healing_check" {
  name               = "auto-healing-check"
  check_interval_sec = 5
  timeout_sec        = 5

  http_health_check {
    port = 80
  }
}

resource "google_pubsub_topic" "scheduler_changes_topic" {
  name = "scheduler-changes-topic"
}

resource "google_cloudfunctions_function" "update_scheduler_ips_function" {
  name        = "update-scheduler-ips-function"
  description = "A function to update scheduler IPs"
  available_memory_mb   = 256
  source_archive_bucket = "some-bucket-name"
  source_archive_object = "function-code.zip"
  trigger_http          = true
  entry_point           = "function_entry_point"
  runtime               = "nodejs14"
  
  event_trigger {
    event_type = "google.pubsub.topic.publish"
    resource = google_pubsub_topic.scheduler_changes_topic.name
  }
}


# --- End Scheduler ---
# --- Begin Worker ---

# Launch group for ARM workers.
resource "google_compute_instance_template" "worker_instance_template" {
  for_each = {
    arm = "arm"
    x86 = "x86"
  }

  name_prefix  = "turbo_cache_worker_template_${each.key}"
  machine_type = "n1-standard-1" # Change to your desired machine type

  instance_description = "worker instance"
  tags                 = ["turbo_cache:instance_type=worker", "turbo_cache:s3_cas_bucket=${google_storage_bucket.cas_bucket.name}", "turbo_cache:scheduler_endpoint=${var.scheduler_domain_prefix}.internal.${var.base_domain}"]

  disk {
    source_image = "turbo_cache_worker_instance_group_manager_x86_2cpu_${each.key}" # Specify the correct image path
    auto_delete  = true
    boot         = true
  }

  network_interface {
    network = "default"
  }

  service_account {
    scopes = ["cloud-platform"]
  }

  lifecycle {
    create_before_destroy = true
  }
}

variable "scheduler_domain_prefix" {
  description = "Scheduler domain prefix"
  type        = string
}

variable "base_domain" {
  description = "Base domain"
  type        = string
}


resource "google_compute_instance_template" "worker_instance_template_arm_1cpu" {
  name_prefix = "turbo_cache_worker_template_arm_1cpu"
  
  machine_type = "ta2a-standard-1" # Adjust to an equivalent GCP machine type
  
  disk {
    source_image = "projects/debian-cloud/global/images/family/debian-11" # Specify the correct image path for ARM
    auto_delete  = true
    boot         = true
  }
  
  network_interface {
    network = "default"
  }
  
  service_account {
    scopes = ["cloud-platform"]
  }
  
  lifecycle {
    create_before_destroy = true
  }
}

resource "google_compute_instance_group_manager" "worker_instance_group_manager_arm_1cpu" {
  name = "turbo_cache_worker_instance_group_manager_arm_1cpu"
  
  version {
    instance_template = google_compute_instance_template.worker_instance_template_arm_1cpu.self_link
  }
  
  target_size       = 1
  base_instance_name = "worker-instance"
  zone              = "us-central1-a" # Adjust to your preferred zone
  
  named_port {
    name = "http"
    port = 80
  }
  
  auto_healing_policies {
    health_check      = google_compute_health_check.default.self_link
    initial_delay_sec = 300
  }
}

resource "google_compute_health_check" "default" {
  name               = "default-health-check"
  check_interval_sec = 120
  timeout_sec        = 5
  
  http_health_check {
    port = 80
  }
}

resource "google_compute_instance_template" "worker_instance_template_x86_2cpu" {
  name_prefix = "turbo_cache_worker_template_x86_2cpu"

  machine_type = "n1-standard-2" # Adjust to an equivalent GCP machine type

  disk {
    source_image = "projects/debian-cloud/global/images/family/debian-11" # Specify the correct image path for x86
    auto_delete  = true
    boot         = true
  }

  network_interface {
    network = "default"
  }

  service_account {
    scopes = ["cloud-platform"]
  }

  lifecycle {
    create_before_destroy = true
  }
}

resource "google_compute_instance_group_manager" "worker_instance_group_manager_x86_2cpu" {
  name = "turbo_cache_worker_instance_group_manager_x86_2cpu"

  version {
    instance_template = "google_compute_instance_template.worker_instance_template_x86_2cpu.self_link"
  }

  target_size       = 1
  base_instance_name = "worker-instance"
  zone              = "us-central1-a" # Adjust to your preferred zone

  named_port {
    name = "http"
    port = 80
  }

  auto_healing_policies {
    health_check      = "google_compute_health_check.default.self_link"
    initial_delay_sec = 300
  }
}

# --- End Worker ---
