resource "google_compute_managed_ssl_certificate" "tfer--tubo-cache-cas-certificate" {
  certificate_id = "3833669532178429162"

  managed {
    domains = ["cas.thirdwave.allada.com"]
  }

  name    = "tubo-cache-cas-certificate"
  project = "turbo-cache-gcp-test"
  type    = "MANAGED"
}

resource "google_compute_managed_ssl_certificate" "tfer--turbo-cache-scheduler-certificate" {
  certificate_id = "455339997646259500"

  managed {
    domains = ["scheduler.thirdwave.allada.com"]
  }

  name    = "turbo-cache-scheduler-certificate"
  project = "turbo-cache-gcp-test"
  type    = "MANAGED"
}
