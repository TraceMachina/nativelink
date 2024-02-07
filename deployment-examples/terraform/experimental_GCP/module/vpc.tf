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

data "google_compute_network" "default" {
  name = "default"
}

resource "google_compute_firewall" "public_firewall" {
  name    = "${var.project_prefix}-public-firewall"
  network = data.google_compute_network.default.id

  allow {
    protocol = "tcp"
    ports    = ["50051"]
  }

  source_ranges = ["0.0.0.0/0"]

  target_service_accounts = [
    google_service_account.browser_service_account.email,
    google_service_account.cas_service_account.email,
    google_service_account.scheduler_service_account.email,
  ]
}

resource "google_compute_firewall" "internal_firewall" {
  name    = "${var.project_prefix}-internal-firewall"
  network = data.google_compute_network.default.id

  allow {
    protocol = "tcp"
    ports    = ["50052"]
  }

  # All traffic from internal traffic.
  source_ranges = [
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
  ]

  target_service_accounts = [
    google_service_account.cas_service_account.email,
    google_service_account.scheduler_service_account.email,
  ]
}

resource "google_compute_firewall" "healthcheck_firewall" {
  name    = "${var.project_prefix}-healthcheck-firewall"
  network = data.google_compute_network.default.id

  allow {
    protocol = "tcp"
    ports = [
      "50051",
    ]
  }

  # GCP health checker ips. See:
  # https://cloud.google.com/load-balancing/docs/health-checks#firewall_rules
  source_ranges = [
    "35.191.0.0/16",
    "130.211.0.0/22",
  ]

  target_service_accounts = [
    google_service_account.browser_service_account.email,
    google_service_account.cas_service_account.email,
    google_service_account.scheduler_service_account.email,
    google_service_account.x86_cpu_worker_service_account.email,
  ]
}

resource "google_compute_firewall" "scheduler_internal_worker_firewall" {
  name    = "${var.project_prefix}-scheduler-internal-worker-firewall"
  network = data.google_compute_network.default.id

  allow {
    protocol = "tcp"
    ports    = ["50061"]
  }

  # Only allow from workers.
  source_service_accounts = [
    google_service_account.x86_cpu_worker_service_account.email,
  ]

  # Only allow to the schedulers.
  target_service_accounts = [
    google_service_account.scheduler_service_account.email,
  ]
}
