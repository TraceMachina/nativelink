resource "google_compute_managed_ssl_certificate" "ssl_certificate" {
  name    = "turbo-cache-ssl-certificate"

  managed {
    domains = [
      "cas.${data.google_dns_managed_zone.dns_zone.dns_name}",
      "scheduler.${data.google_dns_managed_zone.dns_zone.dns_name}"
    ]
  }
}

resource "google_compute_ssl_policy" "lb_ssl_policy" {
  name            = "turbo-cache-lb-ssl-policy"
  min_tls_version = "TLS_1_0"
  profile         = "COMPATIBLE"
}
