resource "google_service_account" "tfer--100748471643570447960" {
  account_id   = "turbo-cache-gcp-test7"
  disabled     = "false"
  display_name = "App Engine default service account"
  project      = "turbo-cache-gcp-test7"
}

resource "google_service_account" "tfer--117834906656421721139" {
  account_id   = "turbo-cache-cf-sa"
  disabled     = "false"
  display_name = "Cloud Functions Service Account"
  project      = "turbo-cache-gcp-test7"
}
