# Copyright 2022 The NativeLink Authors. All rights reserved.
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

variable "build_base_ami_arm" {
  description = "Base AMI for building NativeLink for ARM"
  default     = "ami-0c79a55dda52434da"
}

variable "build_base_ami_x86" {
  description = "Base AMI for building NativeLink for x86"
  default     = "ami-03f65b8614a860c29"
}

variable "build_arm_instance_type" {
  description = "Type of EC2 instance to build NativeLink for ARM"
  default     = "c6gd.2xlarge"
}

variable "build_x86_instance_type" {
  description = "Type of EC2 instance to build NativeLink for x86"
  default     = "c6id.2xlarge"
}

variable "terminate_ami_builder" {
  description = "If we should terminate the AMI builder instances. Disabling this will make development easier, but cost more money."
  default     = true
}

variable "scheduler_instance_type" {
  description = "Type of EC2 instance to use for NativeLink's scheduler"
  default     = "c6g.xlarge"
}

variable "allow_ssh_from_cidrs" {
  description = "List of CIDRs allowed to connect to SSH"
  default     = ["0.0.0.0/0"]
}

variable "base_domain" {
  description = "Base domain name of existing Route53 hosted zone. Subdomains will be added to this zone."
  default     = "nativelink.demo.trace_machina.com"
}

variable "cas_domain_prefix" {
  description = "This will be the DNS name of the cas suffixed with `var.domain`. You may use dot notation to add more sub-domains. Example: `cas.{var.base_domain}`."
  default     = "cas"
}

variable "scheduler_domain_prefix" {
  description = "This will be the DNS name of the scheduler suffixed with `var.domain`. You may use dot notation to add more sub-domains. Example: `cas.{var.base_domain}`."
  default     = "scheduler"
}

# This is a list of Elastic Load Balancing account IDs taken from:
# https://docs.aws.amazon.com/elasticloadbalancing/latest/application/load-balancer-access-logs.html#access-logging-bucket-permissions
# These are the accounts that will publish into the access log bucket.
variable "elb_account_id_for_region_map" {
  description = "Map of ELB account ids that publish into access log bucket."
  default     = {
    "us-east-1": "127311923021",
    "us-east-2": "033677994240",
    "us-west-1": "027434742980",
    "us-west-2": "797873946194",
    "af-south-1": "098369216593",
    "ca-central-1": "985666609251",
    "eu-central-1": "054676820928",
    "eu-west-1": "156460612806",
    "eu-west-2": "652711504416",
    "eu-south-1": "635631232127",
    "eu-west-3": "009996457667",
    "eu-north-1": "897822967062",
    "ap-east-1": "754344448648",
    "ap-northeast-1": "582318560864",
    "ap-northeast-2": "600734575887",
    "ap-northeast-3": "383597477331",
    "ap-southeast-1": "114774131450",
    "ap-southeast-2": "783225319266",
    "ap-southeast-3": "589379963580",
    "ap-south-1": "718504428378",
    "me-south-1": "076674570225",
    "sa-east-1": "507241528517",
    "us-gov-west-1": "048591011584",
    "us-gov-east-1": "190560391635",
    "cn-north-1": "638102146993",
    "cn-northwest-1": "037604701340",
  }
}
