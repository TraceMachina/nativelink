resource "google_dns_record_set" "tfer--turbo-cache-dns-zone_cas-002E-thirdwave-002E-allada-002E-com-002E--A" {
  managed_zone = "google_dns_managed_zone.tfer--turbo-cache-dns-zone.name"
  name         = "cas.google_dns_managed_zone.tfer--turbo-cache-dns-zone.dns_name"
  project      = "turbo-cache-gcp-test"
  rrdatas      = ["34.149.105.12"]
  ttl          = "300"
  type         = "A"
}

resource "google_dns_record_set" "tfer--turbo-cache-dns-zone_internal-002E-scheduler-002E-thirdwave-002E-allada-002E-com-002E--A" {
  managed_zone = "google_dns_managed_zone.tfer--turbo-cache-dns-zone.name"
  name         = "internal.scheduler.google_dns_managed_zone.tfer--turbo-cache-dns-zone.dns_name"
  project      = "turbo-cache-gcp-test"
  rrdatas      = ["10.128.0.14"]
  ttl          = "300"
  type         = "A"
}

resource "google_dns_record_set" "tfer--turbo-cache-dns-zone_scheduler-002E-thirdwave-002E-allada-002E-com-002E--A" {
  managed_zone = "google_dns_managed_zone.tfer--turbo-cache-dns-zone.name"
  name         = "scheduler.google_dns_managed_zone.tfer--turbo-cache-dns-zone.dns_name"
  project      = "turbo-cache-gcp-test"
  rrdatas      = ["34.120.162.95"]
  ttl          = "300"
  type         = "A"
}

resource "google_dns_record_set" "tfer--turbo-cache-dns-zone_thirdwave-002E-allada-002E-com-002E--NS" {
  managed_zone = "google_dns_managed_zone.tfer--turbo-cache-dns-zone.name"
  name         = "google_dns_managed_zone.tfer--turbo-cache-dns-zone.dns_name"
  project      = "turbo-cache-gcp-test"
  rrdatas      = ["ns-cloud-b1.googledomains.com.", "ns-cloud-b2.googledomains.com.", "ns-cloud-b3.googledomains.com.", "ns-cloud-b4.googledomains.com."]
  ttl          = "21600"
  type         = "NS"
}

resource "google_dns_record_set" "tfer--turbo-cache-dns-zone_thirdwave-002E-allada-002E-com-002E--SOA" {
  managed_zone = "google_dns_managed_zone.tfer--turbo-cache-dns-zone.name"
  name         = "google_dns_managed_zone.tfer--turbo-cache-dns-zone.dns_name"
  project      = "turbo-cache-gcp-test"
  rrdatas      = ["ns-cloud-b1.googledomains.com. cloud-dns-hostmaster.google.com. 1 21600 3600 259200 300"]
  ttl          = "21600"
  type         = "SOA"
}
