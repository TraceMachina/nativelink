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

# Note: This is set in the global deployment.
data "google_certificate_manager_certificate_map" "default" {
  name = "${var.project_prefix}-certificate-map"
}

resource "google_compute_ssl_policy" "lb_ssl_policy" {
  name            = "${var.project_prefix}-lb-ssl-policy"
  min_tls_version = "TLS_1_0"
  profile         = "COMPATIBLE"
}
