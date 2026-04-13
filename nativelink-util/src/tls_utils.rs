// Copyright 2024 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use core::time::Duration;

use nativelink_config::stores::{ClientTlsConfig, GrpcEndpoint};
use nativelink_error::{Code, Error, make_err, make_input_err};
use tonic::transport::Uri;
use tracing::{info, warn};

pub fn load_client_config(
    config: &Option<ClientTlsConfig>,
) -> Result<Option<tonic::transport::ClientTlsConfig>, Error> {
    let Some(config) = config else {
        return Ok(None);
    };

    if config.use_native_roots == Some(true) {
        if config.ca_file.is_some() {
            warn!("native root certificates are being used, ca_file will be ignored");
        }
        let tls = tonic::transport::ClientTlsConfig::new().with_native_roots();
        // Apply client identity for mTLS even when using native roots
        let tls = if let Some(client_certificate) = &config.cert_file {
            let Some(client_key) = &config.key_file else {
                return Err(make_err!(
                    Code::Internal,
                    "Client certificate specified, but no key"
                ));
            };
            info!("loading client certificate for mTLS with native roots");
            tls.identity(tonic::transport::Identity::from_pem(
                std::fs::read_to_string(client_certificate)?,
                std::fs::read_to_string(client_key)?,
            ))
        } else {
            if config.key_file.is_some() {
                return Err(make_err!(
                    Code::Internal,
                    "Client key specified, but no certificate"
                ));
            }
            tls
        };
        return Ok(Some(tls));
    }

    let Some(ca_file) = &config.ca_file else {
        return Err(make_err!(
            Code::Internal,
            "CA certificate must be provided if not using native root certificates"
        ));
    };

    let read_config = tonic::transport::ClientTlsConfig::new().ca_certificate(
        tonic::transport::Certificate::from_pem(std::fs::read_to_string(ca_file)?),
    );
    let config = if let Some(client_certificate) = &config.cert_file {
        let Some(client_key) = &config.key_file else {
            return Err(make_err!(
                Code::Internal,
                "Client certificate specified, but no key"
            ));
        };
        read_config.identity(tonic::transport::Identity::from_pem(
            std::fs::read_to_string(client_certificate)?,
            std::fs::read_to_string(client_key)?,
        ))
    } else {
        if config.key_file.is_some() {
            return Err(make_err!(
                Code::Internal,
                "Client key specified, but no certificate"
            ));
        }
        read_config
    };

    Ok(Some(config))
}

pub fn endpoint_from(
    endpoint: &str,
    tls_config: Option<tonic::transport::ClientTlsConfig>,
) -> Result<tonic::transport::Endpoint, Error> {
    let endpoint = Uri::try_from(endpoint)
        .map_err(|e| make_err!(Code::Internal, "Unable to parse endpoint {endpoint}: {e:?}"))?;

    // Tonic uses the TLS configuration if the scheme is "https", so replace
    // grpcs with https.
    let endpoint = if endpoint.scheme_str() == Some("grpcs") {
        let mut parts = endpoint.into_parts();
        parts.scheme = Some("https".parse().map_err(|e| {
            make_err!(
                Code::Internal,
                "https is an invalid scheme apparently? {e:?}"
            )
        })?);
        parts.try_into().map_err(|e| {
            make_err!(
                Code::Internal,
                "Error changing Uri from grpcs to https: {e:?}"
            )
        })?
    } else {
        endpoint
    };

    let endpoint_transport = if let Some(tls_config) = tls_config {
        let Some(authority) = endpoint.authority() else {
            return Err(make_input_err!(
                "Unable to determine authority of endpoint: {endpoint}"
            ));
        };
        if endpoint.scheme_str() != Some("https") {
            return Err(make_input_err!(
                "You have set TLS configuration on {endpoint}, but the scheme is not https or grpcs"
            ));
        }
        let tls_config = tls_config.domain_name(authority.host());
        tonic::transport::Endpoint::from(endpoint)
            .tls_config(tls_config)
            .map_err(|e| make_input_err!("Setting mTLS configuration: {e:?}"))?
    } else {
        if endpoint.scheme_str() == Some("https") {
            return Err(make_input_err!(
                "The scheme of {endpoint} is https or grpcs, but no TLS configuration was provided"
            ));
        }
        tonic::transport::Endpoint::from(endpoint)
    };

    // Always enable TCP_NODELAY to reduce latency on gRPC connections.
    // Nagle's algorithm delays small writes (up to 40ms), which is
    // harmful for gRPC's many small HTTP/2 frames.
    let endpoint_transport = endpoint_transport.tcp_nodelay(true);

    // Set HTTP/2 flow-control windows to match the server defaults (16 MiB
    // stream, 128 MiB connection).  Tonic/h2 defaults to 64 KiB for both,
    // which caps aggregate throughput per connection to ~128 MB/s at 0.5 ms
    // RTT — far below 10 GbE capacity when many streams share a connection.
    let endpoint_transport = endpoint_transport
        .initial_stream_window_size(16 * 1024 * 1024)
        .initial_connection_window_size(128 * 1024 * 1024);

    Ok(endpoint_transport)
}

pub fn endpoint(endpoint_config: &GrpcEndpoint) -> Result<tonic::transport::Endpoint, Error> {
    let endpoint = endpoint_from(
        &endpoint_config.address,
        load_client_config(&endpoint_config.tls_config)?,
    )?;

    let connect_timeout = if endpoint_config.connect_timeout_s > 0 {
        Duration::from_secs(endpoint_config.connect_timeout_s)
    } else {
        Duration::from_secs(30)
    };
    let tcp_keepalive = if endpoint_config.tcp_keepalive_s > 0 {
        Duration::from_secs(endpoint_config.tcp_keepalive_s)
    } else {
        Duration::from_secs(30)
    };
    let http2_keepalive_interval = if endpoint_config.http2_keepalive_interval_s > 0 {
        Duration::from_secs(endpoint_config.http2_keepalive_interval_s)
    } else {
        Duration::from_secs(30)
    };
    let http2_keepalive_timeout = if endpoint_config.http2_keepalive_timeout_s > 0 {
        Duration::from_secs(endpoint_config.http2_keepalive_timeout_s)
    } else {
        Duration::from_secs(20)
    };

    info!(
        address = %endpoint_config.address,
        concurrency_limit = ?endpoint_config.concurrency_limit,
        connect_timeout_s = connect_timeout.as_secs(),
        tcp_keepalive_s = tcp_keepalive.as_secs(),
        http2_keepalive_interval_s = http2_keepalive_interval.as_secs(),
        http2_keepalive_timeout_s = http2_keepalive_timeout.as_secs(),
        "tls_utils::endpoint: creating gRPC endpoint with keepalive",
    );

    let mut endpoint = endpoint
        .connect_timeout(connect_timeout)
        .tcp_nodelay(endpoint_config.tcp_nodelay)
        .tcp_keepalive(Some(tcp_keepalive))
        .http2_keep_alive_interval(http2_keepalive_interval)
        .keep_alive_timeout(http2_keepalive_timeout)
        .keep_alive_while_idle(true)
        // Default to 16 MiB stream window and 128 MiB connection window
        // to avoid capping per-stream throughput at ~64 MB/s with 1ms RTT
        // (hyper's default of 64 KiB is too small for high-bandwidth links).
        .initial_stream_window_size(16 * 1024 * 1024)
        .initial_connection_window_size(128 * 1024 * 1024);

    if let Some(concurrency_limit) = endpoint_config.concurrency_limit {
        endpoint = endpoint.concurrency_limit(concurrency_limit);
    }

    Ok(endpoint)
}

/// Clone-able QUIC/HTTP3 channel for gRPC clients.
///
/// `tonic_h3::H3Channel` wraps a `BoxService` internally and doesn't
/// implement `Clone`, but tonic generated clients require `T: Clone`.
/// We use `tower::buffer::Buffer` which correctly serializes
/// `poll_ready`/`call` pairs through a background worker task,
/// properly routing wakers so concurrent callers don't deadlock.
///
/// Type alias for the inner buffered H3 service.
#[cfg(feature = "quic")]
type H3BufferedService = tower::buffer::Buffer<
    hyper::Request<tonic::body::Body>,
    futures::future::BoxFuture<
        'static,
        Result<
            hyper::Response<
                h3_util::client_body::H3IncomingClient<h3_quinn::RecvStream, bytes::Bytes>,
            >,
            tonic_h3::Error,
        >,
    >,
>;

/// A pool of QUIC/HTTP3 connections that distributes RPCs across
/// multiple independent quinn connections via round-robin. Each
/// connection has its own UDP socket, quinn Endpoint, and Connection
/// mutex, eliminating the single-mutex bottleneck that serializes
/// all streams on one connection.
///
/// `Buffer` is Clone (Arc-backed), so cloning QuicChannel is cheap.
/// Each clone gets its own `selected` index so concurrent clones
/// don't interfere with each other's poll_ready/call pairing.
#[cfg(feature = "quic")]
#[derive(Clone)]
pub struct QuicChannel {
    channels: Vec<H3BufferedService>,
    /// Global round-robin counter shared across all clones.
    counter: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// Index selected by the most recent poll_ready on THIS clone.
    /// Per-clone (not shared) to avoid race between concurrent clones.
    selected: usize,
}

#[cfg(feature = "quic")]
impl std::fmt::Debug for QuicChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicChannel")
            .field("connections", &self.channels.len())
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "quic")]
impl tower::Service<hyper::Request<tonic::body::Body>> for QuicChannel {
    type Response = hyper::Response<
        h3_util::client_body::H3IncomingClient<h3_quinn::RecvStream, bytes::Bytes>,
    >;
    type Error = tower::BoxError;
    type Future = <H3BufferedService as tower::Service<hyper::Request<tonic::body::Body>>>::Future;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        // Only select a new channel when we haven't committed to one yet.
        // On Pending retries, keep polling the same channel to avoid
        // waker misrouting and counter skew.
        if self.selected >= self.channels.len() {
            self.selected = self.counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                % self.channels.len();
        }
        tower::Service::poll_ready(&mut self.channels[self.selected], cx)
    }

    fn call(&mut self, req: hyper::Request<tonic::body::Body>) -> Self::Future {
        let idx = self.selected;
        // Reset so next poll_ready picks a new channel.
        self.selected = usize::MAX;
        tower::Service::call(&mut self.channels[idx], req)
    }
}

/// Create a pool of QUIC/HTTP3 channels for a gRPC endpoint.
///
/// Creates `connections` independent QUIC connections, each with its own
/// UDP socket, quinn Endpoint, and Connection mutex. RPCs are distributed
/// across connections via round-robin, eliminating the single-mutex
/// bottleneck in quinn's Connection state.
#[cfg(feature = "quic")]
pub fn h3_channel(endpoint_config: &GrpcEndpoint, connections: usize) -> Result<QuicChannel, Error> {
    use std::sync::Arc;
    use h3_quinn as _;

    let uri: Uri = endpoint_config
        .address
        .parse()
        .map_err(|e| make_input_err!("Invalid URI for QUIC endpoint: {e:?}"))?;

    let server_name = uri
        .host()
        .ok_or_else(|| make_input_err!("QUIC endpoint URI has no host: {}", uri))?
        .to_string();

    // Resolve hostname to an IPv4 address to avoid IPv6 link-local addresses
    // (fe80::) which require a zone ID and cause QUIC timeouts on Linux when
    // connecting to macOS .local hosts (mDNS returns IPv6 link-local first).
    let uri: Uri = {
        let port = uri.port_u16().unwrap_or(443);
        let resolved_host = std::net::ToSocketAddrs::to_socket_addrs(
            &(server_name.as_str(), port),
        )
        .map_err(|e| make_input_err!("Failed to resolve QUIC host {server_name}: {e:?}"))?
        .find(|addr| addr.is_ipv4())
        .ok_or_else(|| make_input_err!("No IPv4 address found for QUIC host {server_name}"))?;
        let new_uri = format!(
            "{}://{}:{}{}",
            uri.scheme_str().unwrap_or("https"),
            resolved_host.ip(),
            resolved_host.port(),
            uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/"),
        );
        info!(
            %server_name,
            resolved = %resolved_host.ip(),
            "QUIC: resolved hostname to IPv4",
        );
        new_uri
            .parse()
            .map_err(|e| make_input_err!("Failed to parse resolved QUIC URI: {e:?}"))?
    };

    // Build rustls ClientConfig with no server cert verification (internal network,
    // self-signed certs). If the endpoint has a client cert+key in tls_config,
    // present them for mTLS authentication.
    let tls_builder = rustls::ClientConfig::builder_with_provider(
        rustls::crypto::aws_lc_rs::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .map_err(|e| make_err!(Code::Internal, "QUIC TLS version error: {e:?}"))?
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(NoCertVerification(
        rustls::crypto::aws_lc_rs::default_provider(),
    )));

    let mut tls_config = if let Some(tls_cfg) = &endpoint_config.tls_config {
        if let Some(cert_file) = &tls_cfg.cert_file {
            let key_file = tls_cfg.key_file.as_ref().ok_or_else(|| {
                make_err!(
                    Code::Internal,
                    "QUIC client certificate specified but no key file"
                )
            })?;
            use rustls::pki_types::pem::PemObject;
            let cert_pem = std::fs::read(cert_file)
                .map_err(|e| make_err!(Code::Internal, "Could not read QUIC client cert {cert_file}: {e:?}"))?;
            let key_pem = std::fs::read(key_file)
                .map_err(|e| make_err!(Code::Internal, "Could not read QUIC client key {key_file}: {e:?}"))?;
            let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
                rustls::pki_types::CertificateDer::pem_reader_iter(&mut &cert_pem[..])
                    .collect::<Result<_, _>>()
                    .map_err(|e| make_err!(Code::Internal, "Could not parse QUIC client certs: {e:?}"))?;
            let key = rustls::pki_types::PrivateKeyDer::from_pem_reader(&mut &key_pem[..])
                .map_err(|e| make_err!(Code::Internal, "Could not parse QUIC client key: {e:?}"))?;
            info!(
                %cert_file,
                %key_file,
                "QUIC: loading client certificate for mTLS",
            );
            tls_builder
                .with_client_auth_cert(certs, key)
                .map_err(|e| make_err!(Code::Internal, "QUIC client auth cert error: {e:?}"))?
        } else {
            if tls_cfg.key_file.is_some() {
                return Err(make_err!(
                    Code::InvalidArgument,
                    "QUIC client key_file specified without cert_file"
                ));
            }
            tls_builder.with_no_client_auth()
        }
    } else {
        tls_builder.with_no_client_auth()
    };

    tls_config.enable_early_data = true;
    tls_config.alpn_protocols = vec![b"h3".to_vec()];

    let mut client_config = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(tls_config)
            .map_err(|e| make_err!(Code::Internal, "Quinn client config error: {e:?}"))?,
    ));

    // Tune QUIC transport for 10 GbE LAN (~0.5ms RTT).
    // BDP = 1.25 GB/s × 0.5ms ≈ 625 KB. Use generous windows to
    // handle bursts and concurrent streams without flow-control stalls.
    let mut transport = quinn::TransportConfig::default();
    transport.stream_receive_window((16 * 1024 * 1024u32).into()); // 16 MiB per stream
    transport.receive_window((256 * 1024 * 1024u32).into()); // 256 MiB connection
    transport.send_window(256 * 1024 * 1024); // 256 MiB
    transport.max_concurrent_bidi_streams(8192u32.into()); // 8K streams per connection
    transport.max_concurrent_uni_streams(1024u32.into());
    transport.initial_rtt(Duration::from_micros(500)); // 0.5ms LAN RTT
    // Reduce ACK delay from default 25ms to 5ms for LAN.
    let mut ack_freq = quinn::AckFrequencyConfig::default();
    ack_freq.max_ack_delay(Some(Duration::from_millis(5)));
    transport.ack_frequency_config(Some(ack_freq));
    // Idle timeout: 15s. Short enough that dead connections (from server
    // restart) are detected within ~2 keepalive cycles (5s each) plus
    // this timeout, rather than blocking RPCs for the full RPC timeout.
    transport.max_idle_timeout(Some(Duration::from_secs(15).try_into().unwrap()));
    // BBR handles bursty workloads better than Cubic on high-BDP LAN.
    transport.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
    // Send QUIC keepalives every 2s to detect dead connections quickly
    // after server restart. Combined with 15s idle timeout, a dead
    // connection is detected within ~4-6s, triggering H3Connection's
    // built-in reconnection before the RPC timeout (120s) expires.
    transport.keep_alive_interval(Some(Duration::from_secs(2)));
    // Enable QUIC MTU discovery for jumbo frames. Probe up to 8952
    // bytes (9000 jumbo MTU minus 40 IPv6 + 8 UDP headers). Reduces
    // packet rate by ~6x vs default 1452.
    transport.initial_mtu(1200);
    let mut mtu_config = quinn::MtuDiscoveryConfig::default();
    mtu_config.upper_bound(8952);
    transport.mtu_discovery_config(Some(mtu_config));
    client_config.transport_config(Arc::new(transport));

    let connections = connections.max(1);
    let mut channels = Vec::with_capacity(connections);

    for i in 0..connections {
        let udp_socket = std::net::UdpSocket::bind("[::]:0")
            .map_err(|e| make_err!(Code::Internal, "QUIC client UDP bind [{i}]: {e:?}"))?;
        {
            const QUIC_UDP_BUF: usize = 8 * 1024 * 1024;
            let sock_ref = socket2::SockRef::from(&udp_socket);
            if let Err(err) = sock_ref.set_send_buffer_size(QUIC_UDP_BUF) {
                info!(?err, i, "Failed to set QUIC client SO_SNDBUF");
            }
            if let Err(err) = sock_ref.set_recv_buffer_size(QUIC_UDP_BUF) {
                info!(?err, i, "Failed to set QUIC client SO_RCVBUF");
            }
        }

        let mut client_endpoint = quinn::Endpoint::new(
            quinn::EndpointConfig::default(),
            None,
            udp_socket,
            quinn::default_runtime()
                .ok_or_else(|| make_err!(Code::Internal, "No async runtime for QUIC client"))?,
        )
        .map_err(|e| make_err!(Code::Internal, "Failed to create QUIC client endpoint [{i}]: {e:?}"))?;
        client_endpoint.set_default_client_config(client_config.clone());

        let connector = tonic_h3::quinn::H3QuinnConnector::new(
            uri.clone(),
            server_name.clone(),
            client_endpoint,
        );

        let h3_channel = tonic_h3::H3Channel::new(connector, uri.clone());
        // 1024 slots per connection. With N connections, total capacity
        // is N×1024 (e.g., 32×1024 = 32768), sufficient for burst peaks
        // while providing backpressure under transport degradation.
        let buffered = tower::buffer::Buffer::new(h3_channel, 1024);
        channels.push(buffered);
    }

    info!(
        address = %endpoint_config.address,
        connections,
        "tls_utils::h3_channel: created QUIC/HTTP3 connection pool",
    );

    Ok(QuicChannel {
        channels,
        counter: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        selected: usize::MAX, // sentinel: no channel selected yet
    })
}

/// Certificate verifier that accepts any server certificate.
/// Used for internal networks with self-signed certs.
#[cfg(feature = "quic")]
#[derive(Debug)]
struct NoCertVerification(rustls::crypto::CryptoProvider);

#[cfg(feature = "quic")]
impl rustls::client::danger::ServerCertVerifier for NoCertVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0
            .signature_verification_algorithms
            .supported_schemes()
    }
}
