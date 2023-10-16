resource "google_compute_firewall" "tfer--default-allow-health-check" {
  allow {
    protocol = "tcp"
  }

  direction     = "INGRESS"
  disabled      = "false"
  name          = "default-allow-health-check"
  network       = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority      = "1000"
  project       = "turbo-cache-gcp-test"
  source_ranges = ["130.211.0.0/22", "209.85.152.0/22", "209.85.204.0/22", "35.191.0.0/16"]
  target_tags   = ["lb-health-check"]
}

resource "google_compute_firewall" "tfer--default-allow-health-check-ipv6" {
  allow {
    protocol = "tcp"
  }

  direction     = "INGRESS"
  disabled      = "false"
  name          = "default-allow-health-check-ipv6"
  network       = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority      = "1000"
  project       = "turbo-cache-gcp-test"
  source_ranges = ["2600:1901:8001::/48", "2600:2d00:1:b029::/64"]
  target_tags   = ["lb-health-check"]
}

resource "google_compute_firewall" "tfer--default-allow-icmp" {
  allow {
    protocol = "icmp"
  }

  description   = "Allow ICMP from anywhere"
  direction     = "INGRESS"
  disabled      = "false"
  name          = "default-allow-icmp"
  network       = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority      = "65534"
  project       = "turbo-cache-gcp-test"
  source_ranges = ["0.0.0.0/0"]
}

resource "google_compute_firewall" "tfer--default-allow-internal" {
  allow {
    ports    = ["0-65535"]
    protocol = "tcp"
  }

  allow {
    ports    = ["0-65535"]
    protocol = "udp"
  }

  allow {
    protocol = "icmp"
  }

  description   = "Allow internal traffic on the default network"
  direction     = "INGRESS"
  disabled      = "false"
  name          = "default-allow-internal"
  network       = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority      = "65534"
  project       = "turbo-cache-gcp-test"
  source_ranges = ["10.128.0.0/9"]
}

resource "google_compute_firewall" "tfer--default-allow-rdp" {
  allow {
    ports    = ["3389"]
    protocol = "tcp"
  }

  description   = "Allow RDP from anywhere"
  direction     = "INGRESS"
  disabled      = "false"
  name          = "default-allow-rdp"
  network       = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority      = "65534"
  project       = "turbo-cache-gcp-test"
  source_ranges = ["0.0.0.0/0"]
}

resource "google_compute_firewall" "tfer--default-allow-ssh" {
  allow {
    ports    = ["22"]
    protocol = "tcp"
  }

  description   = "Allow SSH from anywhere"
  direction     = "INGRESS"
  disabled      = "false"
  name          = "default-allow-ssh"
  network       = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority      = "65534"
  project       = "turbo-cache-gcp-test"
  source_ranges = ["0.0.0.0/0"]
}

resource "google_compute_firewall" "tfer--turbo-cache-cas-firewall-rule" {
  allow {
    ports    = ["50051-50052"]
    protocol = "tcp"
  }

  direction     = "INGRESS"
  disabled      = "false"
  name          = "turbo-cache-cas-firewall-rule"
  network       = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority      = "1000"
  project       = "turbo-cache-gcp-test"
  source_ranges = ["10.0.0.0/8"]
}

resource "google_compute_firewall" "tfer--turbo-cache-internal-worker-api-firewall-rule" {
  allow {
    ports    = ["50061"]
    protocol = "tcp"
  }

  direction     = "INGRESS"
  disabled      = "false"
  name          = "turbo-cache-internal-worker-api-firewall-rule"
  network       = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority      = "1000"
  project       = "turbo-cache-gcp-test"
  source_ranges = ["0.0.0.0"]
}
