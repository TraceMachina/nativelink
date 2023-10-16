resource "google_storage_bucket_iam_policy" "tfer--foobar-test-ewer2" {
  bucket = "b/foobar-test-ewer2"

  policy_data = <<POLICY
{
  "bindings": [
    {
      "members": [
        "projectEditor:turbo-cache-gcp-test7",
        "projectOwner:turbo-cache-gcp-test7"
      ],
      "role": "roles/storage.legacyBucketOwner"
    },
    {
      "members": [
        "projectViewer:turbo-cache-gcp-test7"
      ],
      "role": "roles/storage.legacyBucketReader"
    },
    {
      "members": [
        "projectEditor:turbo-cache-gcp-test7",
        "projectOwner:turbo-cache-gcp-test7"
      ],
      "role": "roles/storage.legacyObjectOwner"
    },
    {
      "members": [
        "projectViewer:turbo-cache-gcp-test7"
      ],
      "role": "roles/storage.legacyObjectReader"
    }
  ]
}
POLICY
}
