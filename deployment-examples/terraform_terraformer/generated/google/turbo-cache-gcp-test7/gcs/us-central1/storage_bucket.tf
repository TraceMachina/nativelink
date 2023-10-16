resource "google_storage_bucket" "tfer--foobar-test-ewer2" {
  default_event_based_hold    = "false"
  force_destroy               = "false"
  location                    = "US-CENTRAL1"
  name                        = "foobar-test-ewer2"
  project                     = "turbo-cache-gcp-test7"
  requester_pays              = "false"
  storage_class               = "STANDARD"
  uniform_bucket_level_access = "true"
}
