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

# variable "base_domain" {
#   description = "Base domain name of existing Route53 hosted zone. Subdomains will be added to this zone."
#   default     = "turbo-cache.demo.allada.com"
# }

# variable "cas_domain_prefix" {
#   description = "This will be the DNS name of the cas suffixed with `var.domain`. You may use dot notation to add more sub-domains. Example: `cas.{var.base_domain}`."
#   default     = "cas"
# }

# variable "scheduler_domain_prefix" {
#   description = "This will be the DNS name of the scheduler suffixed with `var.domain`. You may use dot notation to add more sub-domains. Example: `cas.{var.base_domain}`."
#   default     = "scheduler"
# }

variable "gcp_project_id" {
  description = "Google Cloud project ID."
  # default     = "turbo-cache-gcp-test7"
  default = "twa-rbe"
}

variable "gcp_region" {
  description = "Google Cloud region."
  # default     = "us-central1"
  default = "us-west1"
}

variable "gcp_zone" {
  description = "Google Cloud zone."
  # default     = "us-central1-c"
  default = "us-west1-b"
}
