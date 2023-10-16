resource "google_compute_health_check" "tfer--turbo-cache-cas-health-checker" {
  check_interval_sec = "5"
  healthy_threshold  = "2"

  http_health_check {
    port         = "50051"
    proxy_header = "NONE"
    request_path = "/status"
    response     = "Ok"
  }

  log_config {
    enable = "false"
  }

  name                = "turbo-cache-cas-health-checker"
  project             = "turbo-cache-gcp-test"
  timeout_sec         = "5"
  unhealthy_threshold = "2"
}

resource "google_compute_health_check" "tfer--turbo-cache-scheduler-health-checker" {
  check_interval_sec = "5"
  healthy_threshold  = "2"

  http_health_check {
    port         = "50052"
    proxy_header = "NONE"
    request_path = "/status"
    response     = "Ok"
  }

  log_config {
    enable = "false"
  }

  name                = "turbo-cache-scheduler-health-checker"
  project             = "turbo-cache-gcp-test"
  timeout_sec         = "5"
  unhealthy_threshold = "2"
}
