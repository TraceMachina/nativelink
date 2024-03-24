# NativeLink Helm Chart Quickstart

### Step 1: Installing Helm Chart

Finally, you can download and install the Helm chart from the `nativelink.tgz` tarball linked below, which deploys the CAS component into the EKS cluster. Additional components for remote execution, such as the scheduler and workers, can be added later.

[Download Helm Chart](https://github.com/TraceMachina/nativelink/tree/main/deployment-examples/eks/Dist)

### Step 2: Customize Configuration

Details around configurable fields can be found at https://docs.nativelink.dev/Configuration

### Step 3: Create an S3 bucket using the AWS CLI or console.

[Create an S3 bucket using the AWS CLI or console.](https://docs.aws.amazon.com/AmazonS3/latest/userguide/create-bucket-overview.html)
```bash
REGION=us-east-2
BUCKET_NAME=TODO-CHANGE-ME
aws s3api create-bucket --bucket ${BUCKET_NAME} --region ${REGION} \
  --create-bucket-configuration "LocationConstraint=${REGION}" \
  --object-ownership BucketOwnerEnforced
```

Expected output:
```json5
{
  "Location": "http://bucket_name_here.s3.amazonaws.com/"
}
```

[Create an IAM role to allow a service account in EKS full access to the bucket](https://docs.aws.amazon.com/eks/latest/userguide/iam-roles-for-service-accounts.html)

Example IAM Role policy doc to grant full access to the NativeLink CAS S3 bucket:
```json5
{
	"Version": "2012-10-17",
	"Statement": [
		{
			"Action": ["s3:*", "s3-object-lambda:*"],
			"Effect": "Allow",
			"Resource": [
				"arn:aws:s3:::BUCKET_NAME_HERE",
				"arn:aws:s3:::BUCKET_NAME_HERE/*"
			]
		}
	]
}
```

Example IAM Trust Policy for the NativeLink EKS Service Account:
```json5
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Sid": "",
      "Effect": "Allow",
      "Principal": { "Federated": "EKS OIDC PROVIDER ARN" },
      "Action": "sts:AssumeRoleWithWebIdentity",
      "Condition": {
        "StringEquals": {
          "EKS OIDC PROVIDER ID: sub": "system:serviceaccount:nativelink:nativelink"
        }
      }
    }
  ]
}
```

### Step 4: Installation Using Helm Chart

<!-- vale off -->
The command below installs our application using the Helm chart tarball that was just downloaded. It ensures the application runs within a Kubernetes namespace. If the namespace doesn't exist, it's created.
<!-- vale on -->

```bash
helm upgrade --install <RELEASE_NAME_HERE> nativelink.tgz --namespace <K8S_NAMESPACE> --create-namespace
```

**Note:** *choose a short release name for the installation, such as `try-nl` or `demo`.*
