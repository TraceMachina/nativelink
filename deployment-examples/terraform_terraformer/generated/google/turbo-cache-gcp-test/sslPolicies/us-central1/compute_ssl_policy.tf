resource "google_compute_ssl_policy" "tfer--turbo-cache-cas-ssl-policy" {
  min_tls_version = "TLS_1_0"
  name            = "turbo-cache-cas-ssl-policy"
  profile         = "COMPATIBLE"
  project         = "turbo-cache-gcp-test"
}

resource "google_compute_ssl_policy" "tfer--turbo-cache-scheduler-ssl-policy" {
  min_tls_version = "TLS_1_0"
  name            = "turbo-cache-scheduler-ssl-policy"
  profile         = "COMPATIBLE"
  project         = "turbo-cache-gcp-test"
}
