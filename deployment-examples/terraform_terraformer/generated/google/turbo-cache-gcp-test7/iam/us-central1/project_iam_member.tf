resource "google_project_iam_member" "tfer--projects-002F-turbo-cache-gcp-test7-002F-roles-002F-turboCacheUpdateIpsCfRoleserviceAccount-003A-turbo-cache-cf-sa-0040-turbo-cache-gcp-test7-002E-iam-002E-gserviceaccount-002E-com" {
  member  = "serviceAccount:turbo-cache-cf-sa@turbo-cache-gcp-test7.iam.gserviceaccount.com"
  project = "turbo-cache-gcp-test7"
  role    = "projects/turbo-cache-gcp-test7/roles/turboCacheUpdateIpsCfRole"
}

resource "google_project_iam_member" "tfer--roles-002F-artifactregistry-002E-serviceAgentserviceAccount-003A-service-551021911287-0040-gcp-sa-artifactregistry-002E-iam-002E-gserviceaccount-002E-com" {
  member  = "serviceAccount:service-551021911287@gcp-sa-artifactregistry.iam.gserviceaccount.com"
  project = "turbo-cache-gcp-test7"
  role    = "roles/artifactregistry.serviceAgent"
}

resource "google_project_iam_member" "tfer--roles-002F-cloudbuild-002E-builds-002E-builderserviceAccount-003A-551021911287-0040-cloudbuild-002E-gserviceaccount-002E-com" {
  member  = "serviceAccount:551021911287@cloudbuild.gserviceaccount.com"
  project = "turbo-cache-gcp-test7"
  role    = "roles/cloudbuild.builds.builder"
}

resource "google_project_iam_member" "tfer--roles-002F-cloudbuild-002E-serviceAgentserviceAccount-003A-service-551021911287-0040-gcp-sa-cloudbuild-002E-iam-002E-gserviceaccount-002E-com" {
  member  = "serviceAccount:service-551021911287@gcp-sa-cloudbuild.iam.gserviceaccount.com"
  project = "turbo-cache-gcp-test7"
  role    = "roles/cloudbuild.serviceAgent"
}

resource "google_project_iam_member" "tfer--roles-002F-cloudfunctions-002E-serviceAgentserviceAccount-003A-service-551021911287-0040-gcf-admin-robot-002E-iam-002E-gserviceaccount-002E-com" {
  member  = "serviceAccount:service-551021911287@gcf-admin-robot.iam.gserviceaccount.com"
  project = "turbo-cache-gcp-test7"
  role    = "roles/cloudfunctions.serviceAgent"
}

resource "google_project_iam_member" "tfer--roles-002F-cloudscheduler-002E-serviceAgentserviceAccount-003A-service-551021911287-0040-gcp-sa-cloudscheduler-002E-iam-002E-gserviceaccount-002E-com" {
  member  = "serviceAccount:service-551021911287@gcp-sa-cloudscheduler.iam.gserviceaccount.com"
  project = "turbo-cache-gcp-test7"
  role    = "roles/cloudscheduler.serviceAgent"
}

resource "google_project_iam_member" "tfer--roles-002F-compute-002E-serviceAgentserviceAccount-003A-service-551021911287-0040-compute-system-002E-iam-002E-gserviceaccount-002E-com" {
  member  = "serviceAccount:service-551021911287@compute-system.iam.gserviceaccount.com"
  project = "turbo-cache-gcp-test7"
  role    = "roles/compute.serviceAgent"
}

resource "google_project_iam_member" "tfer--roles-002F-containerregistry-002E-ServiceAgentserviceAccount-003A-service-551021911287-0040-containerregistry-002E-iam-002E-gserviceaccount-002E-com" {
  member  = "serviceAccount:service-551021911287@containerregistry.iam.gserviceaccount.com"
  project = "turbo-cache-gcp-test7"
  role    = "roles/containerregistry.ServiceAgent"
}

resource "google_project_iam_member" "tfer--roles-002F-editorserviceAccount-003A-551021911287-0040-cloudservices-002E-gserviceaccount-002E-com" {
  member  = "serviceAccount:551021911287@cloudservices.gserviceaccount.com"
  project = "turbo-cache-gcp-test7"
  role    = "roles/editor"
}

resource "google_project_iam_member" "tfer--roles-002F-editorserviceAccount-003A-551021911287-compute-0040-developer-002E-gserviceaccount-002E-com" {
  member  = "serviceAccount:551021911287-compute@developer.gserviceaccount.com"
  project = "turbo-cache-gcp-test7"
  role    = "roles/editor"
}

resource "google_project_iam_member" "tfer--roles-002F-editorserviceAccount-003A-turbo-cache-gcp-test7-0040-appspot-002E-gserviceaccount-002E-com" {
  member  = "serviceAccount:turbo-cache-gcp-test7@appspot.gserviceaccount.com"
  project = "turbo-cache-gcp-test7"
  role    = "roles/editor"
}

resource "google_project_iam_member" "tfer--roles-002F-owneruser-003A-blaise-0040-tracemachina-002E-com" {
  member  = "user:blaise@tracemachina.com"
  project = "turbo-cache-gcp-test7"
  role    = "roles/owner"
}

resource "google_project_iam_member" "tfer--roles-002F-pubsub-002E-serviceAgentserviceAccount-003A-service-551021911287-0040-gcp-sa-pubsub-002E-iam-002E-gserviceaccount-002E-com" {
  member  = "serviceAccount:service-551021911287@gcp-sa-pubsub.iam.gserviceaccount.com"
  project = "turbo-cache-gcp-test7"
  role    = "roles/pubsub.serviceAgent"
}

resource "google_project_iam_member" "tfer--roles-002F-run-002E-serviceAgentserviceAccount-003A-service-551021911287-0040-serverless-robot-prod-002E-iam-002E-gserviceaccount-002E-com" {
  member  = "serviceAccount:service-551021911287@serverless-robot-prod.iam.gserviceaccount.com"
  project = "turbo-cache-gcp-test7"
  role    = "roles/run.serviceAgent"
}

resource "google_project_iam_member" "tfer--roles-002F-storage-002E-objectUserserviceAccount-003A-551021911287-compute-0040-developer-002E-gserviceaccount-002E-com" {
  member  = "serviceAccount:551021911287-compute@developer.gserviceaccount.com"
  project = "turbo-cache-gcp-test7"
  role    = "roles/storage.objectUser"
}
