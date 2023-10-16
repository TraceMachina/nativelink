resource "google_storage_bucket_iam_binding" "tfer--foobar-test-ewer2" {
  bucket  = "b/foobar-test-ewer2"
  members = ["projectEditor:turbo-cache-gcp-test7", "projectOwner:turbo-cache-gcp-test7"]
  role    = "roles/storage.legacyObjectOwner"
}
