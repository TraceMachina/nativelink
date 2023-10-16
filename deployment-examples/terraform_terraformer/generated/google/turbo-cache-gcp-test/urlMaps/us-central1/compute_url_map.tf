resource "google_compute_url_map" "tfer--turbo-cache-cas-load-balancer" {
  default_service = "${data.terraform_remote_state.backendServices.outputs.google_compute_backend_service_tfer--turbo-cache-cas-backend-service_self_link}"
  name            = "turbo-cache-cas-load-balancer"
  project         = "turbo-cache-gcp-test"
}

resource "google_compute_url_map" "tfer--turbo-cache-scheduler-load-balancer" {
  default_service = "${data.terraform_remote_state.backendServices.outputs.google_compute_backend_service_tfer--turbo-cache-scheduler-backend-service_self_link}"
  name            = "turbo-cache-scheduler-load-balancer"
  project         = "turbo-cache-gcp-test"
}
