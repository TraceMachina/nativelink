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

# --- Begin AMI Builder ---

resource "aws_iam_role" "iam_builder_role" {
  name = "nativelink_ami_builder_role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17",
    Statement = [
      {
        Action = "sts:AssumeRole",
        Principal = {
          Service = "ec2.amazonaws.com"
        },
        Effect = "Allow",
        Sid    = ""
      }
    ]
  })

  inline_policy {
    name = "nativelink_terminate_instance_policy"

    policy = jsonencode({
      Version = "2012-10-17"
      Statement = [
        {
          Action   = ["ec2:TerminateInstances"]
          Effect   = "Allow"
          Resource = "arn:aws:ec2:*:${data.aws_caller_identity.current.account_id}:instance/*"
        },
      ]
    })
  }
}

resource "aws_iam_instance_profile" "builder_profile" {
  name = "nativelink_builder_profile"
  role = aws_iam_role.iam_builder_role.name
}

resource "aws_iam_role_policy_attachment" "nativelink_ami_builder_describe_ec2_tags_policy" {
  role       = aws_iam_role.iam_builder_role.name
  policy_arn = aws_iam_policy.describe_ec2_tags_policy.arn
}

# --- End AMI Builder ---
# --- Begin Scheduler ---

resource "aws_iam_role" "iam_scheduler_role" {
  name = "nativelink_scheduler_role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17",
    Statement = [
      {
        Action = "sts:AssumeRole",
        Principal = {
          Service = "ec2.amazonaws.com"
        },
        Effect = "Allow",
        Sid    = ""
      }
    ]
  })
}

resource "aws_iam_instance_profile" "scheduler_profile" {
  name = "nativelink_scheduler_profile"
  role = aws_iam_role.iam_scheduler_role.name
}

resource "aws_iam_role_policy_attachment" "nativelink_scheduler_s3_read_policy_attachment" {
  role       = aws_iam_role.iam_scheduler_role.name
  policy_arn = aws_iam_policy.readonly_s3_access_cas_bucket_policy.arn
}

resource "aws_iam_role_policy_attachment" "nativelink_scheduler_describe_ec2_tags_policy" {
  role       = aws_iam_role.iam_scheduler_role.name
  policy_arn = aws_iam_policy.describe_ec2_tags_policy.arn
}

# --- End Scheduler ---
# --- Begin CAS ---

resource "aws_iam_role" "cas_iam_role" {
  name = "nativelink_cas_role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17",
    Statement = [
      {
        Action = "sts:AssumeRole",
        Principal = {
          Service = "ec2.amazonaws.com"
        },
        Effect = "Allow",
        Sid    = ""
      }
    ]
  })
}

resource "aws_iam_instance_profile" "cas_profile" {
  name = "nativelink_cas_profile"
  role = aws_iam_role.cas_iam_role.name
}

resource "aws_iam_role_policy_attachment" "nativelink_cas_read_s3_policy_attachment" {
  role       = aws_iam_role.cas_iam_role.name
  policy_arn = aws_iam_policy.readonly_s3_access_cas_bucket_policy.arn
}

resource "aws_iam_role_policy_attachment" "nativelink_cas_write_s3_policy_attachment" {
  role       = aws_iam_role.cas_iam_role.name
  policy_arn = aws_iam_policy.write_s3_access_bucket_policy.arn
}

resource "aws_iam_role_policy_attachment" "nativelink_cas_describe_ec2_tags_policy" {
  role       = aws_iam_role.cas_iam_role.name
  policy_arn = aws_iam_policy.describe_ec2_tags_policy.arn
}

# --- End CAS ---
# --- Begin Worker ---
resource "aws_iam_role" "worker_iam_role" {
  name = "nativelink_ami_worker_role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17",
    Statement = [
      {
        Action = "sts:AssumeRole",
        Principal = {
          Service = "ec2.amazonaws.com"
        },
        Effect = "Allow",
        Sid    = ""
      }
    ]
  })

  inline_policy {
    name = "nativelink_terminate_instance_policy"

    policy = jsonencode({
      Version = "2012-10-17"
      Statement = [
        {
          Action   = ["ec2:TerminateInstances"]
          Effect   = "Allow"
          Resource = "arn:aws:ec2:*:${data.aws_caller_identity.current.account_id}:instance/*"
        },
      ]
    })
  }
}

resource "aws_iam_instance_profile" "worker_profile" {
  name = "nativelink_worker_profile"
  role = aws_iam_role.worker_iam_role.name
}

resource "aws_iam_role_policy_attachment" "nativelink_worker_read_s3_policy_attachment" {
  role       = aws_iam_role.worker_iam_role.name
  policy_arn = aws_iam_policy.readonly_s3_access_cas_bucket_policy.arn
}

resource "aws_iam_role_policy_attachment" "nativelink_worker_write_s3_policy_attachment" {
  role       = aws_iam_role.worker_iam_role.name
  policy_arn = aws_iam_policy.write_s3_access_bucket_policy.arn
}

resource "aws_iam_role_policy_attachment" "nativelink_worker_describe_ec2_tags_policy" {
  role       = aws_iam_role.worker_iam_role.name
  policy_arn = aws_iam_policy.describe_ec2_tags_policy.arn
}

# --- End Worker ---
# --- Begin Update Scheduler Lambda ---

resource "aws_iam_role" "update_scheduler_ips_lambda_role" {
  name   = "nativelink_update_scheduler_ips_lambda_role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17",
    Statement = [
      {
        Action = "sts:AssumeRole",
        Principal = {
          Service = "lambda.amazonaws.com"
        },
        Effect = "Allow",
        Sid    = ""
      }
    ]
  })

  inline_policy {
    name = "nativelink_update_scheduler_ips_lambda_policy"

    policy = jsonencode({
        Version = "2012-10-17",
        Statement = [
            {
                Sid = "",
                Effect = "Allow",
                Action = [
                  "autoscaling:DescribeAutoScalingGroups",
                  "ec2:DescribeInstances",
                ],
                Resource = "*"
            },
            {
                Sid = "",
                Effect = "Allow",
                Action = "route53:ChangeResourceRecordSets",
                Resource = data.aws_route53_zone.main.arn
            }
        ]
    })
  }
}

# --- End Update Scheduler Lambda ---
# --- Begin Shared ---

resource "aws_iam_policy" "readonly_s3_access_cas_bucket_policy" {
  name        = "nativelink_readonly_s3_access_cas_bucket"
  description = "Read only policy for cas bucket"

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Action = [
          "s3:GetObject",
          "s3:ListBucket",
          "s3:ListMultipartUploadParts"
        ]
        Effect = "Allow"
        Sid    = ""
        Resource = [
          "${aws_s3_bucket.cas_bucket.arn}/*",
          "${aws_s3_bucket.cas_bucket.arn}"
        ]
      },
    ]
  })
}

resource "aws_iam_policy" "write_s3_access_bucket_policy" {
  name        = "nativelink_write_s3_access_cas_bucket"
  description = "Write only policy for cas bucket"

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Action = [
          "s3:PutObject",
          "s3:AbortMultipartUpload",
          "s3:DeleteObjectVersion",
          "s3:DeleteObject"
        ]
        Effect = "Allow"
        Sid    = ""
        Resource = [
          "${aws_s3_bucket.cas_bucket.arn}/*",
          "${aws_s3_bucket.cas_bucket.arn}"
        ]
      },
    ]
  })
}

resource "aws_iam_policy" "describe_ec2_tags_policy" {
  name        = "nativelink_describe_ec2_tags_policy"
  description = "Allows describe tags on ec2 instances"

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Action   = "ec2:DescribeTags",
        Effect   = "Allow",
        Resource = "*",
        Sid      = ""
      },
    ]
  })
}

# --- End Shared ---
