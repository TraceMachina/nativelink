// Copyright 2026 Trace Machina, Inc. All rights reserved.
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

//! Local DNS fixtures; no external nameserver or cloud credentials required.

use core::net::SocketAddr;
use core::time::Duration;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use aws_smithy_runtime_api::client::http::HttpConnector;
use aws_smithy_runtime_api::client::orchestrator::HttpRequest;
use hickory_resolver::TokioResolver;
use hickory_resolver::config::{LookupIpStrategy, NameServerConfig, ResolverConfig};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use nativelink_config::stores::CommonObjectSpec;
use nativelink_macro::nativelink_test;
use nativelink_store::common_s3_utils::TlsClient;
use nativelink_store::hickory_dns::HickoryDnsResolver;
use nativelink_util::spawn;
use nativelink_util::task::JoinHandleDropGuard;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tower_service::Service;

#[derive(Default)]
struct DnsState {
    queries: AtomicUsize,
    negative: AtomicBool,
    drop_queries: AtomicBool,
    address: AtomicU8,
}

async fn dns_fixture() -> (HickoryDnsResolver, Arc<DnsState>, JoinHandleDropGuard<()>) {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let state = Arc::new(DnsState::default());
    state.address.store(1, Ordering::SeqCst);
    let observed = state.clone();
    let server = spawn!("OSS DNS fixture", async move {
        let mut packet = [0_u8; 2048];
        loop {
            let (length, peer) = socket.recv_from(&mut packet).await.unwrap();
            observed.queries.fetch_add(1, Ordering::SeqCst);
            if observed.drop_queries.load(Ordering::SeqCst) {
                continue;
            }
            let mut end = 12;
            while packet[end] != 0 {
                end += usize::from(packet[end]) + 1;
            }
            end += 5;
            assert!(end <= length);
            let negative = observed.negative.load(Ordering::SeqCst);
            let mut response = vec![
                packet[0],
                packet[1],
                0x81,
                if negative { 0x83 } else { 0x80 },
                0,
                1,
                0,
                if negative { 0 } else { 2 },
                0,
                0,
                0,
                0,
            ];
            response.extend_from_slice(&packet[12..end]);
            if !negative {
                let address = observed.address.load(Ordering::SeqCst);
                for octet in [address, address + 1] {
                    // Compressed name, A / IN, TTL=1, four-byte address.
                    response.extend_from_slice(&[
                        0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 1, 0, 4, 127, 0, 0, octet,
                    ]);
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            socket.send_to(&response, peer).await.unwrap();
        }
    });
    let mut nameserver = NameServerConfig::udp(address.ip());
    nameserver.connections[0].port = address.port();
    let config = ResolverConfig::from_parts(None, vec![], vec![nameserver]);
    let mut builder = TokioResolver::builder_with_config(config, TokioRuntimeProvider::default());
    HickoryDnsResolver::configure_options(builder.options_mut());
    builder.options_mut().ip_strategy = LookupIpStrategy::Ipv4Only;
    builder.options_mut().timeout = Duration::from_millis(200);
    builder.options_mut().attempts = 1;
    (
        HickoryDnsResolver::new(builder.build().unwrap()),
        state,
        server,
    )
}

async fn lookup(resolver: &mut HickoryDnsResolver, name: &str) -> std::io::Result<Vec<SocketAddr>> {
    tokio::time::timeout(Duration::from_secs(3), resolver.call(name.parse().unwrap()))
        .await
        .unwrap()
        .map(Iterator::collect)
}

#[nativelink_test]
async fn concurrent_lookups_share_cache_and_preserve_all_addresses() {
    let (resolver, state, _server) = dns_fixture().await;
    let tasks = (0..20).map(|_| {
        let mut resolver = resolver.clone();
        async move { lookup(&mut resolver, "cached.example.").await.unwrap() }
    });
    for mut addresses in futures::future::join_all(tasks).await {
        addresses.sort_unstable();
        assert_eq!(
            addresses,
            vec![
                SocketAddr::from(([127, 0, 0, 1], 0)),
                SocketAddr::from(([127, 0, 0, 2], 0)),
            ]
        );
    }
    assert_eq!(state.queries.load(Ordering::SeqCst), 1);
}

#[nativelink_test]
async fn positive_ttl_expires_and_refreshes_addresses() {
    let (mut resolver, state, _server) = dns_fixture().await;
    let initial = lookup(&mut resolver, "ttl.example.").await.unwrap();
    assert!(initial.contains(&SocketAddr::from(([127, 0, 0, 1], 0))));
    state.address.store(11, Ordering::SeqCst);
    let cached = lookup(&mut resolver, "ttl.example.").await.unwrap();
    assert!(cached.contains(&SocketAddr::from(([127, 0, 0, 1], 0))));
    assert!(!cached.contains(&SocketAddr::from(([127, 0, 0, 11], 0))));
    assert_eq!(state.queries.load(Ordering::SeqCst), 1);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let refreshed = lookup(&mut resolver, "ttl.example.").await.unwrap();
    assert!(refreshed.contains(&SocketAddr::from(([127, 0, 0, 11], 0))));
    assert!(!refreshed.contains(&SocketAddr::from(([127, 0, 0, 1], 0))));
    assert_eq!(state.queries.load(Ordering::SeqCst), 2);
}

#[nativelink_test]
async fn negative_cache_expires_and_allows_recovery() {
    let (mut resolver, state, _server) = dns_fixture().await;
    state.negative.store(true, Ordering::SeqCst);
    assert!(lookup(&mut resolver, "recover.example.").await.is_err());
    state.negative.store(false, Ordering::SeqCst);
    assert!(lookup(&mut resolver, "recover.example.").await.is_err());
    assert_eq!(state.queries.load(Ordering::SeqCst), 1);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(lookup(&mut resolver, "recover.example.").await.is_ok());
    assert_eq!(state.queries.load(Ordering::SeqCst), 2);
}

#[nativelink_test]
async fn dropped_dns_response_fails_promptly_and_can_recover() {
    let (mut resolver, state, _server) = dns_fixture().await;
    state.drop_queries.store(true, Ordering::SeqCst);
    assert!(lookup(&mut resolver, "timeout.example.").await.is_err());
    state.drop_queries.store(false, Ordering::SeqCst);
    assert!(lookup(&mut resolver, "timeout.example.").await.is_ok());
}

#[nativelink_test]
async fn reconnects_reuse_dns_cache_and_keep_the_original_host() {
    let (resolver, state, _dns_server) = dns_fixture().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = spawn!("DNS HTTP fixture", async move {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            let length = socket.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..length]).to_ascii_lowercase();
            assert!(request.contains(&format!("host: probe.example:{port}")));
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
        }
    });
    let client = TlsClient::with_hickory_resolver(
        &CommonObjectSpec {
            insecure_allow_http: true,
            ..Default::default()
        },
        resolver,
    )
    .unwrap();
    for _ in 0..2 {
        let request = HttpRequest::get(format!("http://probe.example:{port}/")).unwrap();
        let response = tokio::time::timeout(Duration::from_secs(3), client.call(request))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status().as_u16(), 200);
    }
    server.await.unwrap();
    assert_eq!(state.queries.load(Ordering::SeqCst), 1);
}
