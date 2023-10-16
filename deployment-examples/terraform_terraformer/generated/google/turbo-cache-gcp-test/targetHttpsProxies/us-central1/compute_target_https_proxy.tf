resource "google_compute_target_https_proxy" "tfer--turbo-cache-cas-load-balancer-target-proxy" {
  name             = "turbo-cache-cas-load-balancer-target-proxy"
  project          = "turbo-cache-gcp-test"
  proxy_bind       = "false"
  quic_override    = "NONE"
  ssl_certificates = ["https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/global/sslCertificates/tubo-cache-cas-certificate"]
  ssl_policy       = "https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/global/sslPolicies/turbo-cache-cas-ssl-policy"
  url_map          = "${data.terraform_remote_state.urlMaps.outputs.google_compute_url_map_tfer--turbo-cache-cas-load-balancer_self_link}"
}

resource "google_compute_target_https_proxy" "tfer--turbo-cache-scheduler-load-bal-target-proxy" {
  name             = "turbo-cache-scheduler-load-bal-target-proxy"
  project          = "turbo-cache-gcp-test"
  proxy_bind       = "false"
  quic_override    = "NONE"
  ssl_certificates = ["https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/global/sslCertificates/turbo-cache-scheduler-certificate"]
  ssl_policy       = "https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/global/sslPolicies/turbo-cache-scheduler-ssl-policy"
  url_map          = "${data.terraform_remote_state.urlMaps.outputs.google_compute_url_map_tfer--turbo-cache-scheduler-load-balancer_self_link}"
}
