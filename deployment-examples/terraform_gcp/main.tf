# Copyright 2023 The Turbo Cache Authors. All rights reserved.
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

terraform {
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "4.84.0"
    }
  }
  required_version = ">= 0.14.9"
}

provider "google" {
  project = var.gcp_project_id
  region  = var.gcp_region
  zone    = var.gcp_zone
}

data "google_compute_default_service_account" "default" {
}

# # Enable Cloud Run API
# resource "google_project_service" "run" {
#   service            = "run.googleapis.com"
#   disable_on_destroy = false
# }

# # Enable Eventarc API
# resource "google_project_service" "eventarc" {
#   service            = "eventarc.googleapis.com"
#   disable_on_destroy = false
# }

# # Enable Pub/Sub API
# resource "google_project_service" "pubsub" {
#   service            = "pubsub.googleapis.com"
#   disable_on_destroy = false
# }

# resource "google_project_service" "cloud_serviceusage" {
#   project                    = var.gcp_project_id
#   service                    = "compute.googleapis.com"
#   disable_dependent_services = true
#   disable_on_destroy = false
# }

# resource "google_project_service" "cloud_serviceusage" {
#   project                    = var.gcp_project_id
#   service                    = "dns.googleapis.com"
#   disable_dependent_services = true
#   disable_on_destroy = false
# }

# gcloud auth application-default login
# https://console.cloud.google.com/marketplace/product/google/compute.googleapis.com
# https://console.cloud.google.com/marketplace/product/google/dns.googleapis.com
# https://console.cloud.google.com/marketplace/product/google/certificatemanager.googleapis.com
# https://console.cloud.google.com/marketplace/product/google/secretmanager.googleapis.com
# https://console.cloud.google.com/marketplace/product/google/cloudfunctions.googleapis.com
# https://console.cloud.google.com/marketplace/product/google/run.googleapis.com
# https://console.cloud.google.com/marketplace/product/google/cloudbuild.googleapis.com
# https://console.cloud.google.com/marketplace/product/google/cloudscheduler.googleapis.com
# https://console.cloud.google.com/marketplace/product/google/iam.googleapis.com
# bazel build //...  --remote_cache=grpcs://cas.thirdwave.allada.com --remote_executor=grpcs://scheduler.thirdwave.allada.com  --remote_instance_name=main --remote_default_exec_properties=cpu_count=1 -j 4 --remote_timeout=600