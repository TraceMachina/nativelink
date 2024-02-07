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

# --- Begin CAS Certificate ---

resource "aws_acm_certificate" "cas_lb_certificate" {
  domain_name       = "${var.cas_domain_prefix}.${var.base_domain}"
  validation_method = "DNS"

  lifecycle {
    create_before_destroy = true
  }
}

resource "aws_acm_certificate_validation" "cas_lb_certificate_validation" {
  certificate_arn         = aws_acm_certificate.cas_lb_certificate.arn
  validation_record_fqdns = [for record in aws_route53_record.cas_lb_domain_record_cert_verify : record.fqdn]
}

# --- End CAS Certificate ---
# --- Begin Scheduler Certificate ---

resource "aws_acm_certificate" "scheduler_lb_certificate" {
  domain_name       = "${var.scheduler_domain_prefix}.${var.base_domain}"
  validation_method = "DNS"

  lifecycle {
    create_before_destroy = true
  }
}

resource "aws_acm_certificate_validation" "scheduler_lb_certificate_validation" {
  certificate_arn         = aws_acm_certificate.scheduler_lb_certificate.arn
  validation_record_fqdns = [for record in aws_route53_record.scheduler_lb_domain_record_cert_verify : record.fqdn]
}

# --- End Scheduler Certificate ---
