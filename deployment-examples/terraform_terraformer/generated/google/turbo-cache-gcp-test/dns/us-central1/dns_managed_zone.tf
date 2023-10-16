resource "google_dns_managed_zone" "tfer--turbo-cache-dns-zone" {
  dns_name      = "thirdwave.allada.com."
  force_destroy = "false"
  name          = "turbo-cache-dns-zone"
  project       = "turbo-cache-gcp-test"
  visibility    = "public"
}
