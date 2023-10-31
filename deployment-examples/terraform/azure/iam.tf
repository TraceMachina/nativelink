# Copyright 2023 The Turbo Cache Authors. All rights reserved.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#    http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# --- Begin VM Builder ---

data "azurerm_client_config" "example" {}

resource "azurerm_role_definition" "example" {
  name  = "turbo_cache_vm_builder_role"
  scope = "/subscriptions/${data.azurerm_client_config.example.subscription_id}"

  permissions {
    actions     = ["Microsoft.Compute/virtualMachines/delete"]
    not_actions = []
  }

  assignable_scopes = [
    "/subscriptions/${data.azurerm_client_config.example.subscription_id}"
  ]
}

resource "azurerm_role_assignment" "example" {
  principal_id   = data.azurerm_client_config.example.client_id
  role_definition_name = azurerm_role_definition.example.name
  scope          = "/subscriptions/${data.azurerm_client_config.example.subscription_id}"
}

# --- End VM Builder ---
# --- Begin Scheduler ---

resource "azurerm_role_definition" "scheduler" {
  name  = "turbo_cache_scheduler_role"
  scope = "/subscriptions/${data.azurerm_client_config.example.subscription_id}"

  permissions {
    actions     = ["Microsoft.Storage/storageAccounts/blobServices/containers/blobs/read"]
    not_actions = []
  }

  assignable_scopes = [
    "/subscriptions/${data.azurerm_client_config.example.subscription_id}"
  ]
}

# ... Role assignments similar to above

# --- End Scheduler ---
# --- Begin CAS ---

resource "azurerm_role_definition" "cas" {
  name  = "turbo_cache_cas_role"
  scope = "/subscriptions/${data.azurerm_client_config.example.subscription_id}"

  permissions {
    actions     = ["Microsoft.Storage/storageAccounts/blobServices/containers/blobs/read"]
    not_actions = []
  }

  assignable_scopes = [
    "/subscriptions/${data.azurerm_client_config.example.subscription_id}"
  ]
}

# ... Role assignments similar to above

# --- End CAS ---
# --- Begin Worker ---

resource "azurerm_role_definition" "worker" {
  name  = "turbo_cache_worker_role"
  scope = "/subscriptions/${data.azurerm_client_config.example.subscription_id}"

  permissions {
    actions     = ["Microsoft.Compute/virtualMachines/delete"]
    not_actions = []
  }

  assignable_scopes = [
    "/subscriptions/${data.azurerm_client_config.example.subscription_id}"
  ]
}

# ... Role assignments similar to above

# --- End Worker ---
# --- Begin Update Scheduler Function ---

resource "azurerm_function_app" "example" {
  name                       = "turboCacheUpdateSchedulerFuncApp"
  resource_group_name        = azurerm_resource_group.example.name
  location                   = azurerm_resource_group.example.location
  os_type                    = "linux"
  version                    = "~3"
  app_service_plan_id        = azurerm_app_service_plan.example.id
  storage_account_name       = azurerm_storage_account.example.name
  storage_account_access_key = azurerm_storage_account.example.primary_access_key
  
  # ... Other configurations and environment variables
}

# ... Role assignments similar to above

# --- End Update Scheduler Function ---
# --- Begin Shared ---

resource "azurerm_storage_account" "example" {
  name                     = "examplestoracc"
  resource_group_name      = azurerm_resource_group.example.name
  location                 = azurerm_resource_group.example.location
  account_tier             = "Standard"
  account_replication_type = "LRS"

  tags = {
    environment = "dev"
  }
}

# ... Role assignments similar to above

# --- End Shared ---
