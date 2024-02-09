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

resource "google_dns_managed_zone" "default" {
  name        = "${var.project_prefix}-dns-zone"
  dns_name    = "${var.gcp_dns_zone}."
  description = "Base DNS zone for NativeLink resources"
  depends_on  = [google_project_service.dns_api]
}

resource "google_dns_record_set" "dns_authorization_wildcard_certificate" {
  name         = google_certificate_manager_dns_authorization.default.dns_resource_record[0].name
  managed_zone = google_dns_managed_zone.default.name
  type         = google_certificate_manager_dns_authorization.default.dns_resource_record[0].type
  ttl          = 300
  rrdatas      = [google_certificate_manager_dns_authorization.default.dns_resource_record[0].data]
}
