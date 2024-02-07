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

# Note: This is commented out so we can maintain metrics across deployments/upgrades.
resource "google_monitoring_metric_descriptor" "metric_x86_cpu_worker_cpu_total" {
  description  = "Total number of CPU resources available for x86 tasks."
  display_name = "x86_cpu_total"
  type         = "custom.googleapis.com/nativelink/x86_workers/cpu_total"
  metric_kind  = "GAUGE"
  value_type   = "INT64"
}

resource "google_monitoring_metric_descriptor" "metric_x86_cpu_worker_actions_queued" {
  description  = "Total number of CPU resources actions queued for x86 tasks."
  display_name = "x86_actions_queued"
  type         = "custom.googleapis.com/nativelink/x86_workers/actions_queued"
  metric_kind  = "GAUGE"
  value_type   = "INT64"
}

resource "google_monitoring_metric_descriptor" "metric_x86_cpu_worker_actions_active" {
  description  = "Total number of CPU resources actions active for x86 tasks."
  display_name = "x86_actions_active"
  type         = "custom.googleapis.com/nativelink/x86_workers/actions_active"
  metric_kind  = "GAUGE"
  value_type   = "INT64"
}

resource "google_monitoring_metric_descriptor" "metric_x86_cpu_worker_actions_total" {
  description  = "Total number of CPU resources actions for x86 tasks."
  display_name = "x86_actions_total"
  type         = "custom.googleapis.com/nativelink/x86_workers/actions_total"
  metric_kind  = "GAUGE"
  value_type   = "INT64"
}

resource "google_monitoring_metric_descriptor" "metric_x86_cpu_worker_cpu_ratio" {
  description  = "CPU resources actions to cpus available for x86 tasks."
  display_name = "x86_cpu_ratio"
  type         = "custom.googleapis.com/nativelink/x86_workers/cpu_ratio"
  metric_kind  = "GAUGE"
  value_type   = "DOUBLE"
}
