resource "google_compute_autoscaler" "scheduler_autoscaler" {
  name = "turbo-cache-scheduler-autoscaler"
  autoscaling_policy {
    cooldown_period = "60"

    # cpu_utilization {
    #   predictive_method = "NONE"
    #   target            = "0.6"
    # }

    max_replicas = "30"
    min_replicas = "0"
    mode         = "OFF"
  }

  target = google_compute_instance_group_manager.scheduler_instance_group.self_link
}

resource "google_compute_instance_group_manager" "scheduler_instance_group" {
  base_instance_name = "turbo-cache-scheduler-group"
  name               = "turbo-cache-scheduler-instance-group"

  named_port {
    name = "scheduler"
    port = "50051"
  }

  named_port {
    name = "internal-scheduler"
    port = "50061"
  }

  target_size = "1"

  version {
    instance_template = google_compute_instance_template.scheduler_instance_template.self_link
  }

  auto_healing_policies {
    health_check      = google_compute_health_check.scheduler_health_checker.self_link
    initial_delay_sec = 300
  }

  wait_for_instances_status = "STABLE"
}

resource "google_compute_instance_template" "scheduler_instance_template" {
  name = "turbo-cache-scheduler-instance-template"

  machine_type   = "e2-standard-8"
  can_ip_forward = false

  scheduling {
    automatic_restart   = true
    on_host_maintenance = "MIGRATE"
  }

  disk {
    source_image = google_compute_image.base_image.self_link
    auto_delete  = true
    boot         = true
  }

  network_interface {
    network = "default"

    # Give it a public IP.
    access_config {
      network_tier = "PREMIUM"
    }
  }

  tags = [
    "turbo-cache-scheduler"
  ]

  metadata = {
    turbo-cache-type = "scheduler"
    turbo-cache-cas-bucket = google_storage_bucket.cas_s3_bucket.name
    turbo-cache-ac-bucket = google_storage_bucket.cas_s3_bucket.name
    turbo-cache-internal-cas-endpoint = "internal.cas.${trim(data.google_dns_managed_zone.dns_zone.dns_name, ".")}"
  }
}

resource "google_compute_backend_service" "scheduler_backend_service" {
  name             = "turbo-cache-scheduler-backend-service"
  affinity_cookie_ttl_sec = "0"

  backend {
    balancing_mode               = "UTILIZATION"
    capacity_scaler              = "1"
    group                        = google_compute_instance_group_manager.scheduler_instance_group.instance_group
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
  health_checks                   = [
    google_compute_health_check.scheduler_health_checker.self_link,
  ]
  load_balancing_scheme           = "EXTERNAL_MANAGED"
  locality_lb_policy              = "ROUND_ROBIN"

  log_config {
    enable      = "false"
  }

  port_name        = "scheduler"
  protocol         = "HTTP2"
  session_affinity = "NONE"
  timeout_sec      = "30"
}

resource "google_compute_health_check" "scheduler_health_checker" {
  name                = "turbo-cache-scheduler-health-checker"
  check_interval_sec = "5"
  healthy_threshold  = "2"

  http2_health_check {
    port         = "50051"
    request_path = "/status"
    response     = "Ok"
  }

  log_config {
    enable = "false"
  }

  timeout_sec         = "5"
  unhealthy_threshold = "2"
}

resource "google_compute_url_map" "scheduler_url_map" {
  name            = "turbo-cache-scheduler-url-map"
  default_service = google_compute_backend_service.scheduler_backend_service.id
}

resource "google_compute_target_https_proxy" "scheduler_http_proxy" {
  name     = "turbo-cache-scheduler-http-proxy"
  url_map  = google_compute_url_map.scheduler_url_map.id
  ssl_certificates = [google_compute_managed_ssl_certificate.ssl_certificate.id]

  proxy_bind       = "false"
  quic_override    = "NONE"

  ssl_policy       = google_compute_ssl_policy.lb_ssl_policy.self_link
}

resource "google_compute_global_forwarding_rule" "scheduler_forwarding_rule" {
  name                  = "turbo-cache-scheduler-forwarding-rule"
  ip_protocol           = "TCP"
  ip_version            = "IPV4"
  load_balancing_scheme = "EXTERNAL_MANAGED"
  port_range            = "443-443"
  target                = google_compute_target_https_proxy.scheduler_http_proxy.self_link
}

resource "google_dns_record_set" "scheduler_dns_record_set" {
  name = "scheduler.${data.google_dns_managed_zone.dns_zone.dns_name}"
  type = "A"
  ttl  = 300

  managed_zone = data.google_dns_managed_zone.dns_zone.name

  rrdatas = [
    google_compute_global_forwarding_rule.scheduler_forwarding_rule.ip_address
  ]
}

resource "google_compute_firewall" "scheduler_firewall" {
  name        = "turbo-cache-scheduler-firewall"
  network     = "default"

  allow {
    protocol  = "tcp"
    ports     = ["50051"]
  }

  # All traffic from the internet.
  source_ranges = ["0.0.0.0/0"]

  # Only allow to the Scheduler instances.
  target_tags = ["turbo-cache-scheduler"]
}

resource "google_dns_record_set" "scheduler_internal_dns_record_set" {
  name = "internal.scheduler.${data.google_dns_managed_zone.dns_zone.dns_name}"
  type = "A"
  ttl  = 300

  managed_zone = data.google_dns_managed_zone.dns_zone.name

  rrdatas = [
    google_compute_global_forwarding_rule.scheduler_forwarding_rule.ip_address
  ]
}

resource "google_compute_firewall" "scheduler_internal_firewall" {
  name        = "turbo-cache-scheduler-internal-firewall"
  network     = "default"

  allow {
    protocol  = "tcp"
    ports     = ["50061"]
  }

  # All traffic from the internet.
  source_tags = ["turbo-cache-worker"]

  # Only allow to the Scheduler instances.
  target_tags = ["turbo-cache-scheduler"]
}
