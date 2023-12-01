# Copyright 2023 The Native Link Authors. All rights reserved.
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
  description = "Google Cloud project ID."
  default     = "my-gcp-project-id"
}

variable "gcp_region" {
  description = "Google Cloud region."
  default     = "us-central1"
}

variable "gcp_zone" {
  description = "Google Cloud zone."
  default     = "us-central1-b"
}

variable "project_prefix" {
  description = "Prefix all names with this value"
  default     = "nldev"
}
