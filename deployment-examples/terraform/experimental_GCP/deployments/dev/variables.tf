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

variable "gcp_project_id" {
  description = "Google cloud project ID"
  default     = "my-gcp-project-id"
}

variable "gcp_region" {
  description = "Google cloud region"
  default     = "us-central1"
}

variable "gcp_zone" {
  description = "Google cloud zone"
  default     = "us-central1-a"
}

variable "project_prefix" {
  description = "Prefix all names with this value"
  default     = "nldev"
}

variable "base_image_machine_type" {
  description = "Machine type for build image"
  default     = "e2-highcpu-16"
}

variable "browser_machine_type" {
  description = "Machine type for BB Browser"
  default     = "e2-micro"
}

variable "cas_machine_type" {
  description = "Machine type for CAS"
  default     = "e2-highcpu-4"
}

variable "scheduler_machine_type" {
  description = "Machine type for Scheduler"
  default     = "e2-highcpu-8"
}

variable "x86_cpu_worker_machine_type" {
  description = "Machine type for x86 Worker"
  default     = "n2d-standard-8"
}
