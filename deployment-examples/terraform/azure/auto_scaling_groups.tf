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

# --- Begin CAS Load Balancer ---
#TODO (marcussorealheis) in Azure Terraform all resources need to be unique so I need to figure out how to pull these in.
# resource "azurerm_lb" "cas_load_balancer" {
#   name                = "turbo-cache-cas-lb"
#   location            = azurerm_resource_group.main.location
#   resource_group_name = azurerm_resource_group.main.name

#   frontend_ip_configuration {
#     name                 = "publicIPAddress"
#     public_ip_address_id = azurerm_public_ip.example.id
#   }
# }

# resource "azurerm_lb_backend_address_pool" "cas_backend_address_pool" {
#   name                = "turbo-cache-cas-backend-pool"
#   loadbalancer_id     = azurerm_lb.cas_load_balancer.id
# }

# resource "azurerm_lb_probe" "cas_probe" {
#   name                = "turbo-cache-cas-probe"
#   loadbalancer_id     = azurerm_lb.cas_load_balancer.id
#   protocol            = "Http"
#   request_path        = "/"
#   interval_in_seconds = 5
#   number_of_probes    = 2
#   port                = 50051
# }

# resource "azurerm_lb_rule" "cas_lb_rule" {
#   name                           = "turbo-cache-cas-rule"
#   loadbalancer_id                = azurerm_lb.cas_load_balancer.id
#   protocol                       = "Tcp"
#   frontend_port                  = 443
#   backend_port                   = 50051
#   frontend_ip_configuration_name = "publicIPAddress"
#   probe_id                       = azurerm_lb_probe.cas_probe.id
# }

# --- End CAS Load Balancer ---
# --- Begin Scheduler Public Load Balancer ---

# resource "azurerm_lb" "scheduler_load_balancer" {
#   name                = "turbo-cache-scheduler-lb"
#   location            = azurerm_resource_group.main.location
#   resource_group_name = azurerm_resource_group.main.name

#   frontend_ip_configuration {
#     name                 = "publicIPAddress"
#     public_ip_address_id = azurerm_public_ip.example.id
#   }
# }

# resource "azurerm_lb_backend_address_pool" "scheduler_backend_address_pool" {
#   name                = "turbo-cache-scheduler-backend-pool"
#   loadbalancer_id     = azurerm_lb.scheduler_load_balancer.id
# }

# resource "azurerm_lb_probe" "scheduler_probe" {
#   name                = "turbo-cache-scheduler-probe"
#   loadbalancer_id     = azurerm_lb.scheduler_load_balancer.id
#   protocol            = "Http"
#   request_path        = "/"
#   interval_in_seconds = 5
#   number_of_probes    = 5
#   port                = 50052
# }

# resource "azurerm_lb_rule" "scheduler_lb_rule" {
#   name                           = "turbo-cache-scheduler-rule"
#   loadbalancer_id                = azurerm_lb.scheduler_load_balancer.id
#   protocol                       = "Tcp"
#   frontend_port                  = 443
#   backend_port                   = 50052
#   frontend_ip_configuration_name = "publicIPAddress"
#   probe_id                       = azurerm_lb_probe.scheduler_probe.id
# }

# --- End Scheduler Public Load Balancer ---
