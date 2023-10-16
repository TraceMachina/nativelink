resource "google_compute_subnetwork" "tfer--default" {
  ip_cidr_range              = "10.128.0.0/20"
  name                       = "default"
  network                    = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  private_ip_google_access   = "false"
  private_ipv6_google_access = "DISABLE_GOOGLE_ACCESS"
  project                    = "turbo-cache-gcp-test"
  purpose                    = "PRIVATE"
  region                     = "us-central1"
  stack_type                 = "IPV4_ONLY"
}

resource "google_compute_subnetwork" "tfer--test" {
  ip_cidr_range              = "10.99.0.0/16"
  name                       = "test"
  network                    = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  private_ip_google_access   = "false"
  private_ipv6_google_access = "DISABLE_GOOGLE_ACCESS"
  project                    = "turbo-cache-gcp-test"
  purpose                    = "REGIONAL_MANAGED_PROXY"
  region                     = "us-central1"
  role                       = "ACTIVE"
}

resource "google_compute_subnetwork" "tfer--turbo-cache-vpc-network" {
  ip_cidr_range              = "10.128.0.0/20"
  name                       = "turbo-cache-vpc-network"
  network                    = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  private_ip_google_access   = "false"
  private_ipv6_google_access = "DISABLE_GOOGLE_ACCESS"
  project                    = "turbo-cache-gcp-test"
  purpose                    = "PRIVATE"
  region                     = "us-central1"
  stack_type                 = "IPV4_ONLY"
}
