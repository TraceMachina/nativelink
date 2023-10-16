resource "google_service_account" "cf_service_account" {
  account_id   = "turbo-cache-cf-sa"
  display_name = "Cloud Functions Service Account"
}

resource "google_cloudfunctions2_function" "update_ips_cf" {
  name        = "turbo-cache-update-ips-cf"
  location    = var.gcp_region

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
      GCP_PROJECT = var.gcp_project_id,
      DNS_ZONE = data.google_dns_managed_zone.dns_zone.name,
      SCHEDULER_INSTANCE_PREFIX = google_compute_instance_group_manager.scheduler_instance_group.base_instance_name,
      CAS_INSTANCE_PREFIX = google_compute_instance_group_manager.cas_instance_group.base_instance_name,
    }
  }
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
  name        = "turbo-cache-update-ips-invoke-cf"
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
}

resource "google_project_iam_custom_role" "update_ips_cf_role" {
  role_id     = "turboCacheUpdateIpsCfRole"
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
