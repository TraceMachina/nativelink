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

# resource "google_compute_network" "internal_network" {
#   name = "turbo-cache-internal-network"
#   auto_create_subnetworks = false
# }
# # resource "google_compute_subnetwork" "proxy_only_subnet" {
# #   name = "turbo-cache-proxy-only-subnet"
# #   region = var.gcp_region
# # }

# resource "google_compute_subnetwork" "proxy_subnet" {
#   name          = "turbo-cache-proxy-subnet"
#   ip_cidr_range = "172.16.0.0/16"
#   region        = var.gcp_region
#   purpose       = "REGIONAL_MANAGED_PROXY"
#   role          = "ACTIVE"
#   network       = google_compute_network.internal_network.id
# }

# # backend subnet
# resource "google_compute_subnetwork" "internal_subnet" {
#   name          = "turbo-cache-default-subnet"
#   ip_cidr_range = "172.17.0.0/16"
#   region        = var.gcp_region
#   network       = google_compute_network.internal_network.id
# }
