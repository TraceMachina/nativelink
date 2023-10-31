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

resource "tls_private_key" "ssh_key" {
  algorithm = "RSA" 
  rsa_bits  = 2048
}

resource "azurerm_ssh_public_key" "turbo_cache_key" {
  name                = "turbo-cache-key"
  resource_group_name = data.azurerm_resource_group.name
  location            = data.azurerm_resource_group.location
  public_key          = tls_private_key.ssh_key.public_key_openssh
}

data "tls_public_key" "turbo_cache_pem" {
  private_key_pem = tls_private_key.ssh_key.private_key_pem
  # This is left here for convenience. Comment out the line above and uncomment and modify this
  # line to use a custom SSH key.
  # private_key_pem = file(pathexpand("~/.ssh/id_rsa"))
}
