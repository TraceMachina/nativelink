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

resource "google_service_account" "cf_service_account" {
  account_id   = "${var.project_prefix}-cf-sa"
  display_name = "Cloud Functions Service Account"
}

resource "google_cloudfunctions2_function" "update_ips_cf" {
  name     = "${var.project_prefix}-update-ips-cf"
  location = var.gcp_region

  build_config {
    runtime     = "nodejs18"
    entry_point = "updateIPs"
    source {
      storage_source {
        bucket = google_storage_bucket.cf_bucket.name
        object = google_storage_bucket_object.cf_zip_file.name
      }
    }
  }

  service_config {
    min_instance_count    = 1
    available_memory      = "256Mi"
    timeout_seconds       = 60
    service_account_email = google_service_account.cf_service_account.email
    environment_variables = {
      GCP_PROJECT               = var.gcp_project_id,
      DNS_ZONE                  = data.google_dns_managed_zone.dns_zone.name,
      SCHEDULER_INSTANCE_PREFIX = google_compute_region_instance_group_manager.scheduler_instance_group.base_instance_name,
    }
  }
  depends_on = [
    google_project_service.cloud_function_api,
    google_project_service.run_api,
    google_project_service.cloudbuild_api,
  ]
}

resource "google_cloudfunctions2_function_iam_member" "invoker" {
  project        = google_cloudfunctions2_function.update_ips_cf.project
  location       = google_cloudfunctions2_function.update_ips_cf.location
  cloud_function = google_cloudfunctions2_function.update_ips_cf.name
  role           = "roles/cloudfunctions.invoker"
  member         = "serviceAccount:${google_service_account.cf_service_account.email}"
}

resource "google_cloud_run_service_iam_member" "cloud_run_invoker" {
  project  = google_cloudfunctions2_function.update_ips_cf.project
  location = google_cloudfunctions2_function.update_ips_cf.location
  service  = google_cloudfunctions2_function.update_ips_cf.name
  role     = "roles/run.invoker"
  member   = "serviceAccount:${google_service_account.cf_service_account.email}"
}

resource "google_cloud_scheduler_job" "invoke_cloud_function" {
  name        = "${var.project_prefix}-update-ips-invoke-cf"
  description = "Updates the internal IP addresses of some resources in case instances go down."
  schedule    = "*/5 * * * *" # Every 5 mins.
  project     = google_cloudfunctions2_function.update_ips_cf.project
  region      = google_cloudfunctions2_function.update_ips_cf.location

  http_target {
    uri         = google_cloudfunctions2_function.update_ips_cf.service_config[0].uri
    http_method = "POST"
    oidc_token {
      audience              = "${google_cloudfunctions2_function.update_ips_cf.service_config[0].uri}/"
      service_account_email = google_service_account.cf_service_account.email
    }
  }
  depends_on = [google_project_service.cloudscheduler_api]
}

resource "google_project_iam_custom_role" "update_ips_cf_role" {
  role_id     = "${var.project_prefix}UpdateIpsCfRole"
  title       = "Update IPs CF Role"
  description = "Update IPs Cloud Function Role"
  permissions = [
    "compute.instances.list",
    "dns.changes.create",
    "dns.managedZones.get",
    "dns.resourceRecordSets.get",
    "dns.resourceRecordSets.list",
    "dns.resourceRecordSets.update"
  ]
}

resource "google_project_iam_member" "update_ips_cf_iam_member" {
  member  = "serviceAccount:${google_service_account.cf_service_account.email}"
  project = var.gcp_project_id
  role    = google_project_iam_custom_role.update_ips_cf_role.name
}
