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

variable "build_base_image_arm" {
  description = "Base Image for building Turbo Cache for ARM"
  default = {
    publisher = "Canonical"
    offer     = "UbuntuServer"
    sku       = "22_04-lts"
    version   = "latest"
  }
}

variable "build_base_image_x86" {
  description = "Base Image for building Turbo Cache for x86"
  default = {
    publisher = "Canonical"
    offer     = "UbuntuServer"
    sku       = "22_04-lts"
    version   = "latest"
  }
}

variable "build_arm_vm_size" {
  description = "Size of VM instance to build Turbo Cache for ARM"
  default     = "Standard_F8s_v2"
}

variable "build_x86_vm_size" {
  description = "Size of VM instance to build Turbo Cache for x86"
  default     = "Standard_F8s_v2"
}

variable "scheduler_instance_type" {
  description = "Type of EC2 instance to use for Turbo Cache's scheduler"
  default     = "c6g.xlarge"
}

variable "allow_ssh_from_cidrs" {
  description = "List of CIDRs allowed to connect to SSH"
  default     = ["0.0.0.0/0"]
}

variable "base_domain" {
  description = "Base domain name of existing Azure DNS hosted zone. Subdomains will be added to this zone."
  default     = "turbo-cache.demo.monorepository.com"
}

variable "cas_domain_prefix" {
  description = "This will be the DNS name of the cas suffixed with `var.domain`. You may use dot notation to add more sub-domains. Example: `cas.{var.base_domain}`."
  default     = "cas"
}

variable "scheduler_domain_prefix" {
  description = "This will be the DNS name of the scheduler suffixed with `var.domain`. You may use dot notation to add more sub-domains. Example: `cas.{var.base_domain}`."
  default     = "scheduler"
}
