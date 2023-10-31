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

# CAS Blob Container
resource "azurerm_storage_container" "cas_container" {
  name                  = "turbo-cache-cas-container-${data.azurerm_client_config.current.client_id}"
  storage_account_name  = data.azurerm_storage_account.storage.name
  container_access_type = "private"
}

# Access Logs Blob Container
resource "azurerm_storage_container" "access_logs_container" {
  name                  = "turbo-cache-access-logs-${data.azurerm_client_config.current.client_id}"
  storage_account_name  = data.azurerm_storage_account.storage.name
  container_access_type = "private"
}

#TODO (marcussorealheis): Add a retention policy to the access logs container

resource "azurerm_storage_account" "storage_account" {
  name                     = "storageaccount"
  resource_group_name      = data.azurerm_resource_group.existing.name
  location                 = data.azurerm_resource_group.existing.location
  account_tier             = data.azurerm_client_config.current.account_tier
  account_replication_type = "LRS"

  tags = {
    environment = "dev"
  }
}

resource "azurerm_resource_group" "blob" {
  name     = data.azurerm_client_config.current.client_id
  location = data.azurerm_resource_group.existing.location
}

resource "azurerm_storage_container" "allow_blob_access" {
  storage_account_name   = data.azurerm_storage_account.storage.name
  storage_container_name = azurerm_storage_container.access_logs_container.name
  role                   = "Storage Blob Data Contributor"
  principal_id           = data.azurerm_client_config.current.object_id
}
