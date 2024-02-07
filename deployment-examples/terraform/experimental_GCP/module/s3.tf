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

resource "google_storage_bucket" "cas_s3_bucket" {
  name     = "${var.project_prefix}-cas-bucket-${var.gcp_project_id}"
  location = var.gcp_region

  force_destroy = false

  uniform_bucket_level_access = true

  public_access_prevention = "enforced"
  storage_class            = "STANDARD"

  versioning {
    enabled = false
  }
  depends_on = [google_project_service.storage_api]
}

resource "google_storage_bucket" "ac_s3_bucket" {
  name     = "${var.project_prefix}-ac-bucket-${var.gcp_project_id}"
  location = var.gcp_region

  force_destroy = false

  uniform_bucket_level_access = true

  public_access_prevention = "enforced"
  storage_class            = "STANDARD"

  versioning {
    enabled = false
  }
  depends_on = [google_project_service.storage_api]
}

resource "google_storage_bucket" "cf_bucket" {
  name          = "${var.project_prefix}-cf-bucket-${var.gcp_project_id}"
  location      = "US"
  force_destroy = true

  uniform_bucket_level_access = true

  public_access_prevention = "enforced"
  storage_class            = "STANDARD"

  versioning {
    enabled = false
  }
  depends_on = [google_project_service.storage_api]
}

data "archive_file" "cf_zip_file" {
  type        = "zip"
  output_path = "${path.root}/.terraform-nativelink-builder/cf_files.zip"
  source_dir  = "${path.module}/scripts/cloud_functions"
  depends_on  = [google_compute_instance.build_instance]
}

resource "google_storage_bucket_object" "cf_zip_file" {
  name   = "cf_files.zip"
  bucket = google_storage_bucket.cf_bucket.name
  source = data.archive_file.cf_zip_file.output_path
}
