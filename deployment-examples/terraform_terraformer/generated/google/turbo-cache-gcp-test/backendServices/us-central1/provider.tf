provider "google" {
  project = "turbo-cache-gcp-test"
}

terraform {
	required_providers {
		google = {
	    version = "~> 4.0.0"
		}
  }
}
