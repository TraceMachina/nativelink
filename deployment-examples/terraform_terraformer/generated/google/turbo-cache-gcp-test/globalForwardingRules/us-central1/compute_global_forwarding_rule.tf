resource "google_compute_global_forwarding_rule" "tfer--turbo-cache-cas-load-balancer" {
  ip_address            = "34.149.105.12"
  ip_protocol           = "TCP"
  ip_version            = "IPV4"
  load_balancing_scheme = "EXTERNAL_MANAGED"
  name                  = "turbo-cache-cas-load-balancer"
  port_range            = "443-443"
  project               = "turbo-cache-gcp-test"
  target                = "${data.terraform_remote_state.targetHttpsProxies.outputs.google_compute_target_https_proxy_tfer--turbo-cache-cas-load-balancer-target-proxy_self_link}"
}

resource "google_compute_global_forwarding_rule" "tfer--turbo-cache-scheduler-frontned-ip-port" {
  ip_address            = "34.120.162.95"
  ip_protocol           = "TCP"
  ip_version            = "IPV4"
  load_balancing_scheme = "EXTERNAL_MANAGED"
  name                  = "turbo-cache-scheduler-frontned-ip-port"
  port_range            = "443-443"
  project               = "turbo-cache-gcp-test"
  target                = "${data.terraform_remote_state.targetHttpsProxies.outputs.google_compute_target_https_proxy_tfer--turbo-cache-scheduler-load-bal-target-proxy_self_link}"
}
