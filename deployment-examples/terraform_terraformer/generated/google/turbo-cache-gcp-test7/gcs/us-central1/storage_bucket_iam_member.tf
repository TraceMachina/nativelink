resource "google_storage_bucket_iam_member" "tfer--foobar-test-ewer2" {
  bucket = "b/foobar-test-ewer2"
  member = "projectOwner:turbo-cache-gcp-test7"
  role   = "roles/storage.legacyBucketOwner"
}
