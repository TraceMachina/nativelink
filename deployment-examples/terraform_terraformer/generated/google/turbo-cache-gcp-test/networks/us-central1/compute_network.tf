resource "google_compute_network" "tfer--default" {
  auto_create_subnetworks         = "true"
  delete_default_routes_on_create = "false"
  description                     = "Default network for the project"
  mtu                             = "0"
  name                            = "default"
  project                         = "turbo-cache-gcp-test"
  routing_mode                    = "REGIONAL"
}

resource "google_compute_network" "tfer--turbo-cache-vpc-network" {
  auto_create_subnetworks         = "true"
  delete_default_routes_on_create = "false"
  mtu                             = "0"
  name                            = "turbo-cache-vpc-network"
  project                         = "turbo-cache-gcp-test"
  routing_mode                    = "REGIONAL"
}
