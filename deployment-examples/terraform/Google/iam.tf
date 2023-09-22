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

# --- Begin GCI Builder ---

resource "google_project_iam_custom_role" "builder_role" {
  role_id     = "turbo_cache_ami_builder_role"
  title       = "Builder Role"
  permissions = ["compute.instances.get", "compute.instances.list", "compute.instances.terminate"]
}

resource "google_compute_instance_iam_binding" "gce_builder_instance_binding" {
  instance = "turbo_cache_gce_builder_instance"
  role = "roles/compute.instanceAdmin"
}

resource "google_service_account" "builder_service_account" {
  account_id   = "builder-service-account"
  display_name = "Builder Service Account"
}

resource "google_project_iam_binding" "builder_binding" {
  role   = google_project_iam_custom_role.builder_role.id
  members = [
    "serviceAccount:${google_service_account.builder_service_account.email}",
  ]
}

# --- End GCI Builder ---
# --- Begin Scheduler ---

resource "google_project_iam_custom_role" "scheduler_role" {
  role_id = "turbo_cache_scheduler_role"
  title = "Scheduler Role"
  permissions = ["compute.instances.get", "compute.instances.list"]
}

resource "google_service_account" "scheduler_service_account" {
  account_id = "scheduler_service_account"
  display_name = "Scheduler Service Account"
}

resource "google_project_iam_binding" "scheduler_storage_read_binding" {
  role   = "roles/storage.objectViewer"
  members = [
    "serviceAccount:${google_service_account.scheduler_service_account.email}",
  ]
}

resource "google_project_iam_binding" "scheduler_compute_viewer_binding" {
  role   = "roles/compute.viewer"
  members = [
    "serviceAccount:${google_service_account.scheduler_service_account.email}",
  ]
}

# --- End Scheduler ---
# --- Begin CAS ---

resource "aws_iam_role" "cas_iam_role" {
  name = "turbo_cache_cas_role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17",
    Statement = [
      {
        Action = "sts:AssumeRole",
        Principal = {
          Service = "ec2.amazonaws.com"
        },
        Effect = "Allow",
        Sid    = ""
      }
    ]
  })
}

resource "google_service_account" "cas_service_account" {
  account_id   = "cas-service-account"
  display_name = "CAS Service Account"
}

resource "google_project_iam_binding" "cas_storage_read_binding" {
  role   = "roles/storage.objectViewer"
  members = [
    "serviceAccount:${google_service_account.cas_service_account.email}",
  ]
}

resource "google_project_iam_binding" "cas_storage_write_binding" {
  role   = "roles/storage.objectAdmin"
  members = [
    "serviceAccount:${google_service_account.cas_service_account.email}",
  ]
}

resource "google_project_iam_binding" "cas_compute_viewer_binding" {
  role   = "roles/compute.viewer"
  members = [
    "serviceAccount:${google_service_account.cas_service_account.email}",
  ]
}

# --- End CAS ---
# --- Begin Worker ---
resource "google_project_iam_custom_role" "worker_role" {
  role_id     = "turbo_cache_gce_worker_role"
  title       = "Worker Role"
  permissions = ["compute.instances.get", "compute.instances.delete"]
}

resource "google_service_account" "worker_service_account" {
  account_id   = "j-b-gce-worker-service-account"
  display_name = "Worker Service Account"
}

resource "google_project_iam_binding" "worker_binding" {
  role   = google_project_iam_custom_role.worker_role.id
  members = [
    "serviceAccount:${google_service_account.worker_service_account.email}",
  ]
}

resource "google_project_iam_custom_role" "read_gcs_custom_role" {
  role_id     = "turbo_cache_worker_read_gcs_role"
  title       = "Read GCS Bucket Role"
  permissions = ["storage.objects.get", "storage.objects.list"]
}

resource "google_project_iam_binding" "read_gcs_binding" {
  role   = google_project_iam_custom_role.read_gcs_custom_role.id
  members = [
    "serviceAccount:${google_service_account.worker_service_account.email}",
  ]
}


resource "google_project_iam_custom_role" "write_gcs_custom_role" {
  role_id     = "turbo_cache_worker_write_gcs_role"
  title       = "Write GCS Bucket Role"
  permissions = ["storage.objects.create", "storage.objects.delete"]
}

resource "google_project_iam_binding" "write_gcs_binding" {
  role   = google_project_iam_custom_role.write_gcs_custom_role.id
  members = [
    "serviceAccount:${google_service_account.worker_service_account.email}",
  ]
}

resource "google_project_iam_custom_role" "describe_instances_custom_role" {
  role_id     = "turbo_cache_worker_describe_instances_role"
  title       = "Describe Compute Instances Role"
  permissions = ["compute.instances.get"]
}

resource "google_project_iam_binding" "describe_instances_binding" {
  role   = google_project_iam_custom_role.describe_instances_custom_role.id
  members = [
    "serviceAccount:${google_service_account.worker_service_account.email}",
  ]
}

# --- End Worker ---
# --- Begin Update Scheduler Lambda ---

resource "google_service_account" "update_scheduler_ips_function_account" {
  account_id   = "turbo_cache_update_scheduler_ips_function_account"
  display_name = "turbo_cache_update_scheduler_ips_function_account"
}

resource "google_project_iam_custom_role" "update_scheduler_ips_custom_role" {
  role_id     = "turbo_cache_update_scheduler_ips_custom_role"
  title       = "Update Scheduler IPs Custom Role"
  permissions = [
    "compute.instances.list",
    "compute.instances.get",
    "compute.autoscalers.list",
    "dns.changes.create",
    "dns.changes.list",
    "dns.managedZones.list"
  ]
}

resource "google_project_iam_binding" "update_scheduler_ips_binding" {
  role   = google_project_iam_custom_role.update_scheduler_ips_custom_role.id
  members = [
    "serviceAccount:${google_service_account.update_scheduler_ips_function_account.email}",
  ]
}

# --- End Update Scheduler Lambda ---
# --- Begin Shared ---

resource "google_storage_bucket_iam_policy" "write_gcs_access_cas_bucket_policy" {
  bucket = google_storage_bucket.cas_bucket.name

  policy_data = jsonencode({
    bindings = [
      {
        role = "roles/storage.objectAdmin",
        members = [
          "serviceAccount:your-service-account-email@example.iam.gserviceaccount.com"
        ],
        condition = {
          title      = "WriteAccessCondition",
          description = "Write only policy for cas bucket",
          expression  = "request.verb == 'create' || request.verb == 'delete'"
        }
      },
    ]
  })
}

resource "google_project_iam_custom_role" "describe_compute_engine_tags_role" {
  role_id     = "turbo_cache_describe_compute_engine_tags_role"
  title       = "Describe Compute Engine Tags Role"
  description = "Allows describing tags on Compute Engine instances"
  permissions = ["compute.instances.get"]
  project     = "your-gcp-project-id"
}

resource "google_project_iam_policy" "describe_compute_engine_tags_policy" {
  project     = "your-gcp-project-id"
  policy_data = jsonencode({
    bindings = [
      {
        role = "projects/your-gcp-project-id/roles/turbo_cache_describe_compute_engine_tags_role",
        members = [
          "serviceAccount:your-service-account-email@example.iam.gserviceaccount.com"
        ]
      }
    ]
  })
}

# --- End Shared ---
