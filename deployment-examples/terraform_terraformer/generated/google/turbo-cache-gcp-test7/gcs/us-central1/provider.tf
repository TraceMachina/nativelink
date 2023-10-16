provider "google" {
  project = "turbo-cache-gcp-test7"
}

terraform {
	required_providers {
		google = {
	    version = "~> 4.0.0"
		}
  }
}
