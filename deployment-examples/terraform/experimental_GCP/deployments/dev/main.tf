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

terraform {
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "5.5.0"
    }
  }
  required_version = ">= 1.6.3"
}

provider "google" {
  project = var.gcp_project_id
  region  = var.gcp_region
  zone    = var.gcp_zone
}

module "nativelink" {
  source = "../../module"

  gcp_project_id              = var.gcp_project_id
  gcp_region                  = var.gcp_region
  gcp_zone                    = var.gcp_zone
  project_prefix              = var.project_prefix
  base_image_machine_type     = var.base_image_machine_type
  browser_machine_type        = var.browser_machine_type
  cas_machine_type            = var.cas_machine_type
  scheduler_machine_type      = var.scheduler_machine_type
  x86_cpu_worker_machine_type = var.x86_cpu_worker_machine_type
}
