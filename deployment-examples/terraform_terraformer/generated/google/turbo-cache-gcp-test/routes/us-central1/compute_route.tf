resource "google_compute_route" "tfer--default-route-0019527cca558235" {
  description = "Default local route to the subnetwork 10.128.0.0/20."
  dest_range  = "10.128.0.0/20"
  name        = "default-route-0019527cca558235"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-04c1b0461a872b50" {
  description = "Default local route to the subnetwork 10.140.0.0/20."
  dest_range  = "10.140.0.0/20"
  name        = "default-route-04c1b0461a872b50"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-0b4fa768ed09fd63" {
  description = "Default local route to the subnetwork 10.128.0.0/20."
  dest_range  = "10.128.0.0/20"
  name        = "default-route-0b4fa768ed09fd63"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-0d27a7767931ab21" {
  description = "Default local route to the subnetwork 10.190.0.0/20."
  dest_range  = "10.190.0.0/20"
  name        = "default-route-0d27a7767931ab21"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-0f62af207fbbdeff" {
  description = "Default local route to the subnetwork 10.99.0.0/16."
  dest_range  = "10.99.0.0/16"
  name        = "default-route-0f62af207fbbdeff"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-11450d73af23b415" {
  description = "Default local route to the subnetwork 10.202.0.0/20."
  dest_range  = "10.202.0.0/20"
  name        = "default-route-11450d73af23b415"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-123bbb3ec39ce3b3" {
  description = "Default local route to the subnetwork 10.214.0.0/20."
  dest_range  = "10.214.0.0/20"
  name        = "default-route-123bbb3ec39ce3b3"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-130fbf8b17229194" {
  description      = "Default route to the Internet."
  dest_range       = "0.0.0.0/0"
  name             = "default-route-130fbf8b17229194"
  network          = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  next_hop_gateway = "https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/global/gateways/default-internet-gateway"
  priority         = "1000"
  project          = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-1a8c70096159cee2" {
  description = "Default local route to the subnetwork 10.204.0.0/20."
  dest_range  = "10.204.0.0/20"
  name        = "default-route-1a8c70096159cee2"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-209443d4543a5d15" {
  description = "Default local route to the subnetwork 10.146.0.0/20."
  dest_range  = "10.146.0.0/20"
  name        = "default-route-209443d4543a5d15"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-20b9ce9655acfeca" {
  description = "Default local route to the subnetwork 10.148.0.0/20."
  dest_range  = "10.148.0.0/20"
  name        = "default-route-20b9ce9655acfeca"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-23f238ee89feeb70" {
  description = "Default local route to the subnetwork 10.174.0.0/20."
  dest_range  = "10.174.0.0/20"
  name        = "default-route-23f238ee89feeb70"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-2ac230117cd1df88" {
  description = "Default local route to the subnetwork 10.212.0.0/20."
  dest_range  = "10.212.0.0/20"
  name        = "default-route-2ac230117cd1df88"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-2d82f980db9deb5a" {
  description = "Default local route to the subnetwork 10.190.0.0/20."
  dest_range  = "10.190.0.0/20"
  name        = "default-route-2d82f980db9deb5a"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-32bd1b1dee5e163e" {
  description = "Default local route to the subnetwork 10.156.0.0/20."
  dest_range  = "10.156.0.0/20"
  name        = "default-route-32bd1b1dee5e163e"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-3b020815222f933d" {
  description = "Default local route to the subnetwork 10.152.0.0/20."
  dest_range  = "10.152.0.0/20"
  name        = "default-route-3b020815222f933d"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-43ee4e5c9e3993a8" {
  description = "Default local route to the subnetwork 10.154.0.0/20."
  dest_range  = "10.154.0.0/20"
  name        = "default-route-43ee4e5c9e3993a8"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-49c894ede0b94c37" {
  description = "Default local route to the subnetwork 10.174.0.0/20."
  dest_range  = "10.174.0.0/20"
  name        = "default-route-49c894ede0b94c37"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-4a5838035d7c0501" {
  description      = "Default route to the Internet."
  dest_range       = "0.0.0.0/0"
  name             = "default-route-4a5838035d7c0501"
  network          = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  next_hop_gateway = "https://www.googleapis.com/compute/v1/projects/turbo-cache-gcp-test/global/gateways/default-internet-gateway"
  priority         = "1000"
  project          = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-4ed1d3861121ddd7" {
  description = "Default local route to the subnetwork 10.192.0.0/20."
  dest_range  = "10.192.0.0/20"
  name        = "default-route-4ed1d3861121ddd7"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-569760f171824cc3" {
  description = "Default local route to the subnetwork 10.188.0.0/20."
  dest_range  = "10.188.0.0/20"
  name        = "default-route-569760f171824cc3"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-584b0d0f47ece219" {
  description = "Default local route to the subnetwork 10.216.0.0/20."
  dest_range  = "10.216.0.0/20"
  name        = "default-route-584b0d0f47ece219"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-5c571d83169d07eb" {
  description = "Default local route to the subnetwork 10.162.0.0/20."
  dest_range  = "10.162.0.0/20"
  name        = "default-route-5c571d83169d07eb"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-60f44ad6c23f0432" {
  description = "Default local route to the subnetwork 10.140.0.0/20."
  dest_range  = "10.140.0.0/20"
  name        = "default-route-60f44ad6c23f0432"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-63027ceaa856854e" {
  description = "Default local route to the subnetwork 10.206.0.0/20."
  dest_range  = "10.206.0.0/20"
  name        = "default-route-63027ceaa856854e"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-6404671ccb4d7c2f" {
  description = "Default local route to the subnetwork 10.206.0.0/20."
  dest_range  = "10.206.0.0/20"
  name        = "default-route-6404671ccb4d7c2f"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-6405745abb835128" {
  description = "Default local route to the subnetwork 10.202.0.0/20."
  dest_range  = "10.202.0.0/20"
  name        = "default-route-6405745abb835128"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-6a80746134bf063d" {
  description = "Default local route to the subnetwork 10.142.0.0/20."
  dest_range  = "10.142.0.0/20"
  name        = "default-route-6a80746134bf063d"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-71a2fa72cf1c6db1" {
  description = "Default local route to the subnetwork 10.196.0.0/20."
  dest_range  = "10.196.0.0/20"
  name        = "default-route-71a2fa72cf1c6db1"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-7322eec5b40e307a" {
  description = "Default local route to the subnetwork 10.184.0.0/20."
  dest_range  = "10.184.0.0/20"
  name        = "default-route-7322eec5b40e307a"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-747ae95436e6d3cc" {
  description = "Default local route to the subnetwork 10.168.0.0/20."
  dest_range  = "10.168.0.0/20"
  name        = "default-route-747ae95436e6d3cc"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-74b3ac5836647410" {
  description = "Default local route to the subnetwork 10.138.0.0/20."
  dest_range  = "10.138.0.0/20"
  name        = "default-route-74b3ac5836647410"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-7597643f93464df5" {
  description = "Default local route to the subnetwork 10.180.0.0/20."
  dest_range  = "10.180.0.0/20"
  name        = "default-route-7597643f93464df5"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-75f842656d4b6f97" {
  description = "Default local route to the subnetwork 10.198.0.0/20."
  dest_range  = "10.198.0.0/20"
  name        = "default-route-75f842656d4b6f97"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-78dba8d625549dc5" {
  description = "Default local route to the subnetwork 10.212.0.0/20."
  dest_range  = "10.212.0.0/20"
  name        = "default-route-78dba8d625549dc5"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-78f4795a0885e98b" {
  description = "Default local route to the subnetwork 10.160.0.0/20."
  dest_range  = "10.160.0.0/20"
  name        = "default-route-78f4795a0885e98b"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-7b5630320aefa2af" {
  description = "Default local route to the subnetwork 10.182.0.0/20."
  dest_range  = "10.182.0.0/20"
  name        = "default-route-7b5630320aefa2af"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-811e27ead9653a85" {
  description = "Default local route to the subnetwork 10.182.0.0/20."
  dest_range  = "10.182.0.0/20"
  name        = "default-route-811e27ead9653a85"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-81d94c40e4b0296d" {
  description = "Default local route to the subnetwork 10.178.0.0/20."
  dest_range  = "10.178.0.0/20"
  name        = "default-route-81d94c40e4b0296d"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-88b2b1507351cbdd" {
  description = "Default local route to the subnetwork 10.164.0.0/20."
  dest_range  = "10.164.0.0/20"
  name        = "default-route-88b2b1507351cbdd"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-932d9bdcf55c85aa" {
  description = "Default local route to the subnetwork 10.166.0.0/20."
  dest_range  = "10.166.0.0/20"
  name        = "default-route-932d9bdcf55c85aa"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-944d73c3bbe4a542" {
  description = "Default local route to the subnetwork 10.172.0.0/20."
  dest_range  = "10.172.0.0/20"
  name        = "default-route-944d73c3bbe4a542"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-94b68415dd658b77" {
  description = "Default local route to the subnetwork 10.170.0.0/20."
  dest_range  = "10.170.0.0/20"
  name        = "default-route-94b68415dd658b77"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-977f4717c5fbc87e" {
  description = "Default local route to the subnetwork 10.158.0.0/20."
  dest_range  = "10.158.0.0/20"
  name        = "default-route-977f4717c5fbc87e"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-9c23b905591382cd" {
  description = "Default local route to the subnetwork 10.164.0.0/20."
  dest_range  = "10.164.0.0/20"
  name        = "default-route-9c23b905591382cd"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-a16b0daf6b88178f" {
  description = "Default local route to the subnetwork 10.156.0.0/20."
  dest_range  = "10.156.0.0/20"
  name        = "default-route-a16b0daf6b88178f"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-a36d64d39b876f97" {
  description = "Default local route to the subnetwork 10.186.0.0/20."
  dest_range  = "10.186.0.0/20"
  name        = "default-route-a36d64d39b876f97"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-a4e3a84be3161c8d" {
  description = "Default local route to the subnetwork 10.170.0.0/20."
  dest_range  = "10.170.0.0/20"
  name        = "default-route-a4e3a84be3161c8d"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-ab048d63612ba245" {
  description = "Default local route to the subnetwork 10.172.0.0/20."
  dest_range  = "10.172.0.0/20"
  name        = "default-route-ab048d63612ba245"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-ab39b87b212a8fbe" {
  description = "Default local route to the subnetwork 10.158.0.0/20."
  dest_range  = "10.158.0.0/20"
  name        = "default-route-ab39b87b212a8fbe"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-ab9385ef85d6dc28" {
  description = "Default local route to the subnetwork 10.154.0.0/20."
  dest_range  = "10.154.0.0/20"
  name        = "default-route-ab9385ef85d6dc28"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-abdcf310bfbcc666" {
  description = "Default local route to the subnetwork 10.138.0.0/20."
  dest_range  = "10.138.0.0/20"
  name        = "default-route-abdcf310bfbcc666"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-ae6256954a08a830" {
  description = "Default local route to the subnetwork 10.132.0.0/20."
  dest_range  = "10.132.0.0/20"
  name        = "default-route-ae6256954a08a830"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-b2708d57016f4f2a" {
  description = "Default local route to the subnetwork 10.210.0.0/20."
  dest_range  = "10.210.0.0/20"
  name        = "default-route-b2708d57016f4f2a"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-b6e5571795be85be" {
  description = "Default local route to the subnetwork 10.194.0.0/20."
  dest_range  = "10.194.0.0/20"
  name        = "default-route-b6e5571795be85be"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-b8bd8288f34b3af1" {
  description = "Default local route to the subnetwork 10.216.0.0/20."
  dest_range  = "10.216.0.0/20"
  name        = "default-route-b8bd8288f34b3af1"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-ba7e250887fc459f" {
  description = "Default local route to the subnetwork 10.166.0.0/20."
  dest_range  = "10.166.0.0/20"
  name        = "default-route-ba7e250887fc459f"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-bd809d27e9e38ec7" {
  description = "Default local route to the subnetwork 10.132.0.0/20."
  dest_range  = "10.132.0.0/20"
  name        = "default-route-bd809d27e9e38ec7"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-bde6f6e56d9db8f0" {
  description = "Default local route to the subnetwork 10.186.0.0/20."
  dest_range  = "10.186.0.0/20"
  name        = "default-route-bde6f6e56d9db8f0"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-be9f9751c6838016" {
  description = "Default local route to the subnetwork 10.146.0.0/20."
  dest_range  = "10.146.0.0/20"
  name        = "default-route-be9f9751c6838016"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-bf9aa3cfa955dabe" {
  description = "Default local route to the subnetwork 10.208.0.0/20."
  dest_range  = "10.208.0.0/20"
  name        = "default-route-bf9aa3cfa955dabe"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-c3a3c7b942e4668b" {
  description = "Default local route to the subnetwork 10.196.0.0/20."
  dest_range  = "10.196.0.0/20"
  name        = "default-route-c3a3c7b942e4668b"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-c4c0aedd05649e09" {
  description = "Default local route to the subnetwork 10.160.0.0/20."
  dest_range  = "10.160.0.0/20"
  name        = "default-route-c4c0aedd05649e09"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-c5b7159a556b3b73" {
  description = "Default local route to the subnetwork 10.200.0.0/20."
  dest_range  = "10.200.0.0/20"
  name        = "default-route-c5b7159a556b3b73"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-c7392ed4ed916150" {
  description = "Default local route to the subnetwork 10.214.0.0/20."
  dest_range  = "10.214.0.0/20"
  name        = "default-route-c7392ed4ed916150"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-cbd3a22d4d33724c" {
  description = "Default local route to the subnetwork 10.188.0.0/20."
  dest_range  = "10.188.0.0/20"
  name        = "default-route-cbd3a22d4d33724c"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-cd8deeb48c8faaab" {
  description = "Default local route to the subnetwork 10.150.0.0/20."
  dest_range  = "10.150.0.0/20"
  name        = "default-route-cd8deeb48c8faaab"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-cdb382bdea58f8dc" {
  description = "Default local route to the subnetwork 10.194.0.0/20."
  dest_range  = "10.194.0.0/20"
  name        = "default-route-cdb382bdea58f8dc"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-d8d9dd1063724e9b" {
  description = "Default local route to the subnetwork 10.204.0.0/20."
  dest_range  = "10.204.0.0/20"
  name        = "default-route-d8d9dd1063724e9b"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-dc9fea81f7d63c95" {
  description = "Default local route to the subnetwork 10.208.0.0/20."
  dest_range  = "10.208.0.0/20"
  name        = "default-route-dc9fea81f7d63c95"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-ddad1406950e05c3" {
  description = "Default local route to the subnetwork 10.148.0.0/20."
  dest_range  = "10.148.0.0/20"
  name        = "default-route-ddad1406950e05c3"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-dde7e7d6f77ee8e9" {
  description = "Default local route to the subnetwork 10.178.0.0/20."
  dest_range  = "10.178.0.0/20"
  name        = "default-route-dde7e7d6f77ee8e9"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-de2aaf11809bd4d5" {
  description = "Default local route to the subnetwork 10.142.0.0/20."
  dest_range  = "10.142.0.0/20"
  name        = "default-route-de2aaf11809bd4d5"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-dfea5d2d870107c0" {
  description = "Default local route to the subnetwork 10.150.0.0/20."
  dest_range  = "10.150.0.0/20"
  name        = "default-route-dfea5d2d870107c0"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-e7bb08c2376e687d" {
  description = "Default local route to the subnetwork 10.184.0.0/20."
  dest_range  = "10.184.0.0/20"
  name        = "default-route-e7bb08c2376e687d"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-e7da7ae7c709f674" {
  description = "Default local route to the subnetwork 10.192.0.0/20."
  dest_range  = "10.192.0.0/20"
  name        = "default-route-e7da7ae7c709f674"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-e804a95236a48a39" {
  description = "Default local route to the subnetwork 10.162.0.0/20."
  dest_range  = "10.162.0.0/20"
  name        = "default-route-e804a95236a48a39"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-f2e9e10d836e018d" {
  description = "Default local route to the subnetwork 10.152.0.0/20."
  dest_range  = "10.152.0.0/20"
  name        = "default-route-f2e9e10d836e018d"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-f38935219ca1d61b" {
  description = "Default local route to the subnetwork 10.168.0.0/20."
  dest_range  = "10.168.0.0/20"
  name        = "default-route-f38935219ca1d61b"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-f41a3502199084a7" {
  description = "Default local route to the subnetwork 10.198.0.0/20."
  dest_range  = "10.198.0.0/20"
  name        = "default-route-f41a3502199084a7"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-f633ad749f51f2f5" {
  description = "Default local route to the subnetwork 10.180.0.0/20."
  dest_range  = "10.180.0.0/20"
  name        = "default-route-f633ad749f51f2f5"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--default_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-fa2e97cfe590fe04" {
  description = "Default local route to the subnetwork 10.200.0.0/20."
  dest_range  = "10.200.0.0/20"
  name        = "default-route-fa2e97cfe590fe04"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}

resource "google_compute_route" "tfer--default-route-fc87f4918ddb7405" {
  description = "Default local route to the subnetwork 10.210.0.0/20."
  dest_range  = "10.210.0.0/20"
  name        = "default-route-fc87f4918ddb7405"
  network     = "${data.terraform_remote_state.networks.outputs.google_compute_network_tfer--turbo-cache-vpc-network_self_link}"
  priority    = "0"
  project     = "turbo-cache-gcp-test"
}
