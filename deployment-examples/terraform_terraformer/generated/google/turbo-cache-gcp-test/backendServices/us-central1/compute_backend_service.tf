resource "google_compute_backend_service" "tfer--turbo-cache-cas-backend-service" {
  affinity_cookie_ttl_sec = "0"

  backend {
    balancing_mode               = "UTILIZATION"
    capacity_scaler              = "1"
    group                        = "${data.terraform_remote_state.instanceGroupManagers.outputs.google_compute_instance_group_manager_tfer--turbo-cache-cas-instance-group_instance_group}"
    max_connections              = "0"
    max_connections_per_endpoint = "0"
    max_connections_per_instance = "0"
    max_rate                     = "0"
    max_rate_per_endpoint        = "0"
    max_rate_per_instance        = "0"
    max_utilization              = "0.8"
  }

  connection_draining_timeout_sec = "300"
  enable_cdn                      = "false"
  health_checks                   = ["${data.terraform_remote_state.healthChecks.outputs.google_compute_health_check_tfer--turbo-cache-cas-health-checker_self_link}"]
  load_balancing_scheme           = "EXTERNAL_MANAGED"
  locality_lb_policy              = "ROUND_ROBIN"

  log_config {
    enable      = "false"
    sample_rate = "0"
  }

  name             = "turbo-cache-cas-backend-service"
  port_name        = "cas"
  project          = "turbo-cache-gcp-test"
  protocol         = "HTTP"
  session_affinity = "NONE"
  timeout_sec      = "30"
}

resource "google_compute_backend_service" "tfer--turbo-cache-scheduler-backend-service" {
  affinity_cookie_ttl_sec = "0"

  backend {
    balancing_mode               = "RATE"
    capacity_scaler              = "1"
    group                        = "${data.terraform_remote_state.instanceGroupManagers.outputs.google_compute_instance_group_manager_tfer--turbo-cache-scheduler-instance-group_instance_group}"
    max_connections              = "0"
    max_connections_per_endpoint = "0"
    max_connections_per_instance = "0"
    max_rate                     = "0"
    max_rate_per_endpoint        = "0"
    max_rate_per_instance        = "100000"
    max_utilization              = "0"
  }

  cdn_policy {
    cache_key_policy {
      include_host         = "true"
      include_protocol     = "true"
      include_query_string = "true"
    }

    cache_mode                   = "CACHE_ALL_STATIC"
    client_ttl                   = "3600"
    default_ttl                  = "3600"
    max_ttl                      = "86400"
    negative_caching             = "false"
    serve_while_stale            = "0"
    signed_url_cache_max_age_sec = "0"
  }

  connection_draining_timeout_sec = "300"
  enable_cdn                      = "false"
  health_checks                   = ["${data.terraform_remote_state.healthChecks.outputs.google_compute_health_check_tfer--turbo-cache-scheduler-health-checker_self_link}"]
  load_balancing_scheme           = "EXTERNAL_MANAGED"
  locality_lb_policy              = "ROUND_ROBIN"

  log_config {
    enable      = "false"
    sample_rate = "0"
  }

  name             = "turbo-cache-scheduler-backend-service"
  port_name        = "scheduler"
  project          = "turbo-cache-gcp-test"
  protocol         = "HTTP"
  session_affinity = "NONE"
  timeout_sec      = "30"
}
