resource "google_project_iam_custom_role" "tfer--projects-002F-turbo-cache-gcp-test7-002F-roles-002F-turboCacheUpdateIpsCfRole" {
  description = "Update IPs Cloud Function Role"
  permissions = ["compute.instances.list"]
  project     = "turbo-cache-gcp-test7"
  role_id     = "turboCacheUpdateIpsCfRole"
  stage       = "GA"
  title       = "Update IPs CF Role"
}
