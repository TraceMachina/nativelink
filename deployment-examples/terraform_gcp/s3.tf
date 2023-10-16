resource "google_storage_bucket" "cas_s3_bucket" {
  name        = "turbo-cache-cas-bucket-${var.gcp_project_id}"
  location      = var.gcp_region
  # TODO(allada) Remove this.
  force_destroy = true

  uniform_bucket_level_access = true

  public_access_prevention = "enforced"
  storage_class = "STANDARD"

  versioning {
    enabled = false
  }
}

resource "google_storage_hmac_key" "key" {
  service_account_email = data.google_compute_default_service_account.default.email
}

resource "google_storage_bucket" "cf_bucket" {
  name                        = "turbo-cache-cf-bucket-${var.gcp_project_id}"
  location                    = "US"
  force_destroy = true

  uniform_bucket_level_access = true

  public_access_prevention = "enforced"
  storage_class = "STANDARD"

  versioning {
    enabled = false
  }
}

data "archive_file" "cf_zip_file" {
  type        = "zip"
  output_path = "${path.module}/.terraform-turbo-cache-builder/cf_files.zip"
  source_dir = "${path.module}/scripts/cloud_functions"
}

resource "google_storage_bucket_object" "cf_zip_file" {
  name   = "cf_files.zip"
  bucket = google_storage_bucket.cf_bucket.name
  source = data.archive_file.cf_zip_file.output_path
}
