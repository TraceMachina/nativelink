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

# Note: This call will cost $20 every time it is called.
resource "google_certificate_manager_certificate" "default" {
  name = "${var.project_prefix}-wildcard-ssl-certificate"

  managed {
    domains = [
      "*.${google_certificate_manager_dns_authorization.default.domain}",
      "${google_certificate_manager_dns_authorization.default.domain}"
    ]
    dns_authorizations = [
      google_certificate_manager_dns_authorization.default.id
    ]
  }
}

resource "google_certificate_manager_dns_authorization" "default" {
  name       = "${var.project_prefix}-dns-authorization"
  domain     = var.gcp_dns_zone
  depends_on = [google_project_service.certificate_manager_api]
}

resource "google_certificate_manager_certificate_map" "default" {
  name       = "${var.project_prefix}-certificate-map"
  depends_on = [google_project_service.certificate_manager_api]
}

resource "google_certificate_manager_certificate_map_entry" "default_certificate_entry" {
  name         = "${var.project_prefix}-default-domain-entry"
  description  = "${google_certificate_manager_dns_authorization.default.domain} certificate entry"
  map          = google_certificate_manager_certificate_map.default.name
  certificates = [google_certificate_manager_certificate.default.id]
  hostname     = google_certificate_manager_dns_authorization.default.domain
}

resource "google_certificate_manager_certificate_map_entry" "sub_domain_certificate_entry" {
  name         = "${var.project_prefix}-sub-domain-entry"
  description  = "*.${google_certificate_manager_dns_authorization.default.domain} certificate entry"
  map          = google_certificate_manager_certificate_map.default.name
  certificates = [google_certificate_manager_certificate.default.id]
  hostname     = "*.${google_certificate_manager_dns_authorization.default.domain}"
}
