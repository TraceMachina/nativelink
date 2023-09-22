# Copyright 2022 The Turbo Cache Authors. All rights reserved.
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

# --- Begin AMI Builder ---

resource "google_compute_firewall" "gci_builder_instance_firewall" {
  name = "turbo_cache_gci_builder_instance_firewall"
  network = "default"

  allow {
    protocol = "all"
  }

  source_ranges = ["0.0.0.0/0"]
}

# --- End AMI Builder Instance ---
# --- Begin Scheduler Instances ---

resource "google_compute_firewall" "schedulers_instance_firewall" {
  name = "turbo_cache_schedulers_instance_firewall"
  network = "default"

  allow {
    protocol = "tcp"
    ports = ["50052"]
  }

  source_ranges = ["0.0.0.0/0"]
}

resource "google_compute_firewall_rule" "inbound_from_workers_to_schedulers_grpc" {
  name = "inbound_from_workers_to_schedulers_grpc"
  firewall = google_compute_firewall.schedulers_instance_firewall.id
  priority = 100

  action = "allow"
  protocol = "tcp"
  ports = ["50061"]
  source_filter_tags = ["worker-instance"]
}

# --- End Scheduler Instances ---
# --- Begin Scheduler Load Balancers ---

resource "google_compute_firewall" "schedulers_load_balancer_firewall" {
  name = "turbo_cache_schedulers_load_balancer_firewall"
  network = "default"

  allow {
    protocol = "tcp"
    ports = ["443"]
  }

  source_ranges = ["0.0.0.0/0"]
}

# --- End Scheduler Load Balancers ---
# --- Begin CAS Instances ---

resource "google_compute_firewall" "cas_instance_firewall" {
  name = "turbo_cache_cas_instance_firewall"
  network = "default"

  allow {
    protocol = "tcp"
    ports = ["50051"]
  }

  source_ranges = ["0.0.0.0/0"]
}

# --- End CAS Instances ---
# --- Begin Worker Instances ---

resource "google_compute_firewall" "worker_instance_firewall" {
  name = "turbo_cache_worker_instance_firewall"
  network = "default"

  allow {
    protocol = "tcp"
    ports = ["50061"]
  }

  source_ranges = ["0.0.0.0/0"]
}

resource "google_compute_firewall_rule" "outbound_worker_to_schedulers_rule" {
  name = "outbound_worker_to_schedulers_rule"
  firewall = google_compute_firewall.worker_instance_firewall.id
  priority = 100

  action = "allow"
  protocol = "tcp"
  ports = ["50061"]
  target_tags = ["scheduler-instance"]
}

# --- End Worker Instances ---
# --- Begin Other ---

resource "google_compute_firewall" "aws_api_ec2_endpoint_firewall" {
  name = "turbo_cache_aws_api_ec2_endpoint_firewall"
  network = "default"

  allow {
    protocol = "tcp"
    ports = ["443"]
  }

  source_tags = ["aws-ec2-endpoint"]
}

resource "google_compute_firewall" "allow_ssh_firewall" {
  name = "turbo_cache_allow_ssh_firewall"
  network = "default"

  allow {
    protocol = "tcp"
    ports = ["22"]
  }

  source_ranges = ["0.0.0.0/0"]
}

# --- End Other ---