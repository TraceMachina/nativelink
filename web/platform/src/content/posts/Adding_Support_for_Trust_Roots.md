---
title: "Trust Root Support in Nativelink"
tags: ["news", "blog-posts"]
image: https://private-user-images.githubusercontent.com/2353608/446805624-b91afe27-f186-4d7a-bb1a-2902f6ec0362.png?jwt=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJnaXRodWIuY29tIiwiYXVkIjoicmF3LmdpdGh1YnVzZXJjb250ZW50LmNvbSIsImtleSI6ImtleTUiLCJleHAiOjE3NDc5NjEzMDUsIm5iZiI6MTc0Nzk2MTAwNSwicGF0aCI6Ii8yMzUzNjA4LzQ0NjgwNTYyNC1iOTFhZmUyNy1mMTg2LTRkN2EtYmIxYS0yOTAyZjZlYzAzNjIucG5nP1gtQW16LUFsZ29yaXRobT1BV1M0LUhNQUMtU0hBMjU2JlgtQW16LUNyZWRlbnRpYWw9QUtJQVZDT0RZTFNBNTNQUUs0WkElMkYyMDI1MDUyMyUyRnVzLWVhc3QtMSUyRnMzJTJGYXdzNF9yZXF1ZXN0JlgtQW16LURhdGU9MjAyNTA1MjNUMDA0MzI1WiZYLUFtei1FeHBpcmVzPTMwMCZYLUFtei1TaWduYXR1cmU9NmY5NmE5ZGFlYjc2ZDgzYzMxNzA5Mjg1NTkwOGVhZjc1MzNjNTEyODg5NzY0Nzc5NzhlMjliZjE2NTU5MzE2NSZYLUFtei1TaWduZWRIZWFkZXJzPWhvc3QifQ.hHz8no8ugtOp0oKdI9zOd3ihk7h83x1kT-jJjPk-oB8
slug: adding-trust-roots-to-nativelink
pubDate: 2025-05-15
readTime: 8 minutes
---

# Native Root Certificate Support in Nativelink

**Open source thrives on community contributions, and today's story is a perfect example of why.** External engineer [Sam Eskandar](https://github.com/s6eskand) identified a real pain point in Nativelink's TLS configuration and delivered [a straightforward and elegant solution](https://github.com/TraceMachina/nativelink/pull/1782) that fundamentally improves how developers can deploy and scale their build systems. This contribution showcases the collaborative spirit that makes open source projects stronger and more accessible to everyone.

## The Problem: TLS Configuration Complexity

Before this enhancement, connecting to gRPC endpoints using TLS in Nativelink required manual configuration of certificate files - specifically providing CA certificate files, certificate files, and key files. This created significant friction for teams using managed certificates or deploying in environments where distributing secrets across workers was either impractical or posed security concerns.

Consider the previous [`TlsConfig`](https://nativelink.com/docs/reference/nativelink-config/#clienttlsconfig) structure:

```rust
{
  cert_file: "example_string",
  key_file: "example_string",
  client_ca_file: null,
  client_crl_file: null
}
```

Every TLS connection demanded explicit certificate management, forcing developers to handle certificate distribution, rotation, and storage manually. For teams leveraging cloud-managed certificates or certificate authorities, this approach created unnecessary operational overhead and potential security vulnerabilities.

## The Solution: Native Root Certificate Support

The new implementation introduces a `use_native_roots` option in the `ClientTlsConfig`. When enabled, Nativelink automatically leverages the system's native root certificate store, eliminating the need for manual certificate file management in many common deployment scenarios.

### Technical Implementation Details

The core changes improve Nativelink's TLS utilities with certificate handling logic:

```rust
if config.use_native_roots == Some(true) {
    if config.ca_file.is_some() {
        warn!("Native root certificates are being used, all certificate files will be ignored");
    }
    return Ok(Some(
        tonic::transport::ClientTlsConfig::new().with_native_roots(),
    ));
}
```

This implementation provides three distinct behaviors:

1. **Native roots enabled** (`use_native_roots: true`): Uses system native root certificates, ignoring any provided certificate files
2. **Traditional certificate files**: Uses the existing manual certificate file location configuration
3. **Validation and error handling**: Guides users with clear error messages

The changes span multiple components:

- **Configuration structures**: Improved `ClientTlsConfig` with the new `use_native_roots` field
- **TLS utilities**: Updated certificate loading logic with native root support
- **Build configuration**: Added `tls-native-roots` feature to tonic dependency
- **Comprehensive testing**: New test suite covering all configuration scenarios

### Deployment Flexibility Improvements

This enhancement dramatically improves Nativelink's deployment model flexibility:

**Cloud-Native Deployments**: Teams using managed Kubernetes clusters, cloud load balancers with managed certificates, or service meshes like Istio can now connect seamlessly without certificate file distribution.

**Enterprise Environments**: Organizations with established PKI infrastructure and corporate certificate authorities can leverage existing trust chains without additional configuration overhead.

**Development Workflows**: Local development environments can connect to staging or production endpoints using system trust stores, reducing development friction.

## Configuration Examples

### Using Native Root Certificates

For the majority of modern deployments, you can now enable native roots with one parameter:

```json
{
  "tls_config": {
    "use_native_roots": true
  }
}
```

This configuration automatically trusts certificates signed by any certificate authority in your system's trust store - perfect for cloud environments with managed certificates.

### Traditional Certificate-Based Configuration

For environments requiring specific certificate validation:

```json
{
  "tls_config": {
    "use_native_roots": false,
    "ca_file": "/path/to/ca.pem",
    "cert_file": "/path/to/client.pem",
    "key_file": "/path/to/client-key.pem"
  }
}
```

### Hybrid Cloud Deployments

The new flexibility shines in hybrid scenarios where different components might require different TLS approaches:

```json
{
  "stores": {
    "production_cache": {
      "endpoint": "https://cache.production.company.com",
      "tls_config": {
        "use_native_roots": true
      }
    },
    "internal_service": {
      "endpoint": "https://internal.service:8443",
      "tls_config": {
        "use_native_roots": false,
        "ca_file": "/path/to/ca.pem",
        "cert_file": "/path/to/client.pem",
        "key_file": "/path/to/client-key.pem"
      }
    }
  }
}
```

**Simplified Operations**: Production deployments require fewer configuration steps and eliminate certificate distribution concerns.

**Enhanced Security Posture**: Leveraging system trust stores reduces the attack surface compared to manually managed certificate files scattered across deployment artifacts.

**Better Developer Experience**: Local development environments can seamlessly connect to remote services without complex certificate bootstrapping.

## The Open Source Advantage

This contribution exemplifies why open source creates superior software. An engineer outside the core team identified a real-world pain point, designed an elegant solution, and contributed back to the community. The collaborative review process - involving multiple maintainers providing technical feedback, usability insights, and future roadmap considerations - resulted in a more robust implementation than any single organization might have developed in isolation.

The pull request discussion reveals the thoughtful engineering culture: considerations about error handling for invalid certificates, documentation needs, and integration with other Nativelink components like S3 storage. This collaborative refinement process is open source at its finest.

## Looking Forward

With native root certificate support, Nativelink becomes significantly more accessible to teams across diverse infrastructure environments. Whether you're running containerized workloads in Kubernetes, deploying on traditional virtual machines with corporate PKI, or prototyping locally, TLS configuration no longer presents a barrier to adoption.

The contribution also highlights areas for future enhancement: improved error messaging for certificate validation failures, documentation expanding TLS configuration patterns, and potential integration with cloud-specific certificate management services.

**This is the power of open source in action** - community-driven improvements that make technology more accessible, secure, and flexible for everyone building the future. We're grateful for contributions like this that strengthen Nativelink's position as the premier open source remote execution platform.

---

*Interested in contributing to Nativelink? Check out our [GitHub repository](https://github.com/TraceMachina/nativelink) and join our growing community of developers building the future of distributed build systems.*
