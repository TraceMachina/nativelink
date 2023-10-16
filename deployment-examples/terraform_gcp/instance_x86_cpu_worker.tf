resource "google_compute_region_autoscaler" "x86_cpu_worker_autoscaler" {
  name = "turbo-cache-x86-cpu-worker-autoscaler"
  autoscaling_policy {
    cooldown_period = "60"

    # cpu_utilization {
    #   predictive_method = "NONE"
    #   target            = "0.6"
    # }

    max_replicas = "30"
    min_replicas = "1"
    mode         = "OFF"
  }

  target = google_compute_region_instance_group_manager.x86_cpu_worker_instance_group.id
}

resource "google_compute_region_instance_group_manager" "x86_cpu_worker_instance_group" {
  base_instance_name = "turbo-cache-x86-cpu-worker-group"
  name               = "turbo-cache-x86-cpu-worker-instance-group"
  target_size        = "1"

  version {
    instance_template = google_compute_region_instance_template.x86_cpu_worker_instance_template.id
  }

  auto_healing_policies {
    health_check      = google_compute_health_check.x86_cpu_worker_health_checker.self_link
    initial_delay_sec = 300
  }

  wait_for_instances_status = "STABLE"
  depends_on = [
    google_compute_region_instance_template.x86_cpu_worker_instance_template
  ]
}

resource "google_compute_health_check" "x86_cpu_worker_health_checker" {
  name                = "turbo-cache-x86-cpu-worker-health-checker"
  check_interval_sec = "5"
  healthy_threshold  = "2"

  http_health_check {
    port         = "50061"
    request_path = "/status"
    response     = "Ok"
  }

  log_config {
    enable = "false"
  }

  timeout_sec         = "5"
  unhealthy_threshold = "2"
}

resource "google_compute_region_instance_template" "x86_cpu_worker_instance_template" {
  name = "turbo-cache-x86-cpu-worker-instance-template"

  # machine_type   = "e2-standard-8"
  machine_type   = "n2d-standard-8"
  can_ip_forward = false

  scheduling {
    automatic_restart   = false
    provisioning_model = "SPOT"
    preemptible = true
  }

  service_account {
    # Google recommends custom service accounts that have cloud-platform scope and permissions granted via IAM Roles.
    email  = data.google_service_account.x86_cpu_worker_service_account.email
    scopes = ["cloud-platform"]
  }

  disk {
    source_image = google_compute_image.base_image.id
    auto_delete  = true
    boot         = true
  }

  disk {
    disk_type    = "local-ssd"
    type         = "SCRATCH"
    disk_size_gb = "375"
  }

  network_interface {
    network = "default"

    # Give it a public IP.
    access_config {
      network_tier = "PREMIUM"
    }
  }

  # network_interface {
  #   network = google_compute_network.internal_network.id
  #   subnetwork = google_compute_subnetwork.internal_subnet.id
  # }

  tags = [
    "turbo-cache-worker"
  ]

  metadata = {
    turbo-cache-type = "worker"
    turbo-cache-cas-bucket = google_storage_bucket.cas_s3_bucket.name
    turbo-cache-ac-bucket = google_storage_bucket.cas_s3_bucket.name
    turbo-cache-internal-scheduler-endpoint = "internal.scheduler.${trim(data.google_dns_managed_zone.dns_zone.dns_name, ".")}"
    turbo-cache-internal-cas-endpoint = "internal.cas.${trim(data.google_dns_managed_zone.dns_zone.dns_name, ".")}"
  }
}

data "google_service_account" "x86_cpu_worker_service_account" {
  account_id   = "turbo-cache-x86-worker-sa"
}

# resource "google_service_account" "x86_cpu_worker_service_account" {
#   account_id   = "turbo-cache-x86-worker-sa"
#   lifecycle {
#     prevent_destroy = true
#   }
# }

resource "google_project_iam_member" "x86_cpu_worker_artifactory_iam_member" {
  member  = "serviceAccount:${data.google_service_account.x86_cpu_worker_service_account.email}"
  project = var.gcp_project_id
  role    = "roles/artifactregistry.reader"
}


# resource "google_project_iam_custom_role" "worker_role" {
#   role_id     = "turboCacheWorkerRole"
#   title       = "Role for Turbo Cache Worker"
#   description = "General permissions given to the worker instances"
#   permissions = [
#     "compute.snapshots.list",
#   ]
# }

# resource "google_project_iam_member" "worker_iam_member" {
#   member  = "serviceAccount:${data.google_service_account.x86_cpu_worker_service_account.email}"
#   project = var.gcp_project_id
#   role    = google_project_iam_custom_role.worker_role.name
# }


# resource "google_storage_bucket_iam_member" "docker_cache" {
#   bucket = google_storage_bucket.docker_cache_bucket.name
#   role = "roles/storage.objectUser"
#   member  = "serviceAccount:${data.google_service_account.x86_cpu_worker_service_account.email}"
# }


# sudo apt-get install docker.io -y
# gcloud auth configure-docker --quiet
# gcloud auth print-access-token | sudo docker login -u oauth2accesstoken --password-stdin https://gcr.io
# sudo docker pull gcr.io/voltaic-day-213421/devel:f8f17c2f686c9c8280c3d5323de2b496056fe26395ae80f8e3544b7854e8e910

# curl -s "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token" -H "Metadata-Flavor: Google"

# gcloud auth print-access-token | sudo docker login -u oauth2accesstoken --password-stdin https://gcr.io
# sudo docker pull gcr.io/voltaic-day-213421/devel:f8f17c2f686c9c8280c3d5323de2b496056fe26395ae80f8e3544b7854e8e910