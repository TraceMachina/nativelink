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

use core::future::Future;
use core::net::SocketAddr;
use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;
use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, Weak};

use hickory_resolver::TokioResolver;
use hickory_resolver::config::ResolverOpts;
use hyper_util::client::legacy::connect::dns::Name;
use nativelink_error::{Code, Error, make_err};
use tower_service::Service;

const LOOKUP_DEADLINE: Duration = Duration::from_secs(5);
const MAX_PENDING_NAMES: usize = 256;

type NameLocks = Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>;

struct ResolverState {
    resolver: TokioResolver,
    pending: NameLocks,
}

/// A cached DNS resolver shared by connector clones. Concurrent lookups of
/// the same name share a lock so waiting callers reuse cacheable results.
#[derive(Clone)]
pub struct HickoryDnsResolver(Arc<ResolverState>);

impl core::fmt::Debug for HickoryDnsResolver {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HickoryDnsResolver").finish_non_exhaustive()
    }
}

impl HickoryDnsResolver {
    /// Wrap a resolver configured by the caller, for example a local test DNS server.
    pub fn new(resolver: TokioResolver) -> Self {
        Self(Arc::new(ResolverState {
            resolver,
            pending: Mutex::new(HashMap::new()),
        }))
    }

    /// Applies bounded query and cache settings while preserving the system's
    /// nameservers, search domains, hosts-file policy and address-family strategy.
    pub fn configure_options(options: &mut ResolverOpts) {
        options.timeout = Duration::from_secs(2);
        options.attempts = 2;
        options.cache_size = 256;
        options.try_tcp_on_error = true;
        options.positive_max_ttl = Some(Duration::from_secs(300));
        options.negative_min_ttl = Some(Duration::from_secs(1));
        options.negative_max_ttl = Some(Duration::from_secs(30));
    }

    pub(crate) fn shared_system() -> Result<Self, Error> {
        // Weak ownership releases the cache when no clients use it, including
        // when a test runtime ends. Live stores share the same cache.
        static SHARED: Mutex<Weak<ResolverState>> = Mutex::new(Weak::new());
        let mut shared = SHARED
            .lock()
            .map_err(|_| make_err!(Code::Internal, "OSS DNS resolver lock poisoned"))?;
        if let Some(state) = shared.upgrade() {
            return Ok(Self(state));
        }
        let mut builder = TokioResolver::builder_tokio()
            .map_err(|_| make_err!(Code::InvalidArgument, "Unable to read system DNS configuration for OSS; configure system DNS or select dns_resolver: system"))?;
        Self::configure_options(builder.options_mut());
        let resolver = builder.build().map_err(|_| {
            make_err!(
                Code::InvalidArgument,
                "Invalid OSS Hickory DNS configuration"
            )
        })?;
        let resolver = Self::new(resolver);
        *shared = Arc::downgrade(&resolver.0);
        Ok(resolver)
    }

    fn name_lock(&self, name: &str) -> io::Result<Arc<tokio::sync::Mutex<()>>> {
        let mut pending = self
            .0
            .pending
            .lock()
            .map_err(|_| io::Error::other("OSS DNS lookup lock poisoned"))?;
        pending.retain(|_, lock| lock.strong_count() != 0);
        if let Some(lock) = pending.get(name).and_then(Weak::upgrade) {
            return Ok(lock);
        }
        if pending.len() >= MAX_PENDING_NAMES {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Too many distinct OSS DNS lookups",
            ));
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        pending.insert(name.to_owned(), Arc::downgrade(&lock));
        Ok(lock)
    }
}

impl Service<Name> for HickoryDnsResolver {
    type Response = std::vec::IntoIter<SocketAddr>;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<Self::Response>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, name: Name) -> Self::Future {
        let this = self.clone();
        Box::pin(async move {
            // DNS names are case-insensitive. The HTTP URI, Host and TLS SNI
            // remain untouched; only DNS lookup coalescing uses this key.
            let name = name.as_str().to_ascii_lowercase();
            let lock = this.name_lock(&name)?;
            tokio::time::timeout(LOOKUP_DEADLINE, async {
                let _guard = lock.lock().await;
                let lookup = this
                    .0
                    .resolver
                    .lookup_ip(name)
                    .await
                    .map_err(io::Error::other)?;
                let addresses: Vec<_> = lookup
                    .into_iter()
                    .map(|ip| SocketAddr::new(ip, 0))
                    .collect();
                if addresses.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "OSS DNS returned no addresses",
                    ));
                }
                Ok(addresses.into_iter())
            })
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "OSS DNS lookup deadline exceeded")
            })?
        })
    }
}
