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

# --- Begin Endpoints ---

resource "aws_vpc_endpoint" "aws_api_ec2_endpoint" {
  vpc_id            = data.aws_vpc.main.id
  service_name      = "com.amazonaws.${data.aws_region.current.name}.ec2"
  vpc_endpoint_type = "Interface"

  security_group_ids = [aws_security_group.aws_api_ec2_endpoint_sg.id]

  private_dns_enabled = true
  subnet_ids          = data.aws_subnet_ids.main.ids
}

resource "aws_vpc_endpoint" "aws_api_s3_endpoint" {
  vpc_id            = data.aws_vpc.main.id
  service_name      = "com.amazonaws.${data.aws_region.current.name}.s3"
  vpc_endpoint_type = "Gateway"
  route_table_ids   = [aws_route_table.aws_api_s3_endpoint.id]
}

resource "aws_route_table" "aws_api_s3_endpoint" {
  vpc_id = data.aws_vpc.main.id
}

# --- End Endpoints ---
