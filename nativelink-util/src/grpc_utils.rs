// Copyright 2024 The Native Link Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use async_lock::{Semaphore, SemaphoreGuard};
use nativelink_error::{Code, Error};
use parking_lot::Mutex;
use tonic::transport::{Channel, Endpoint};

/// A helper utility that enables management of a suite of connections to an
/// upstream gRPC endpoint using Tonic.
pub struct ConnectionManager {
    /// The endpoints to establish Channels for.
    endpoints: Vec<Endpoint>,
    /// A balance channel over the above endpoints which is kept with a
    /// monotonic index to ensure we only re-create a channel on the first error
    /// received on it.
    channel: Mutex<(usize, Channel)>,
    /// If a maximum number of upstream requests are allowed at a time, this
    /// is a Semaphore to manage that.
    request_semaphore: Option<Semaphore>,
}

impl ConnectionManager {
    /// Create a connection manager that creates a balance list between a given
    /// set of Endpoints.  This will restrict the number of concurrent requests
    /// assuming that the user of this connection manager uses the connection
    /// only once and reports all errors.
    pub fn new(endpoints: impl IntoIterator<Item = Endpoint>, max_concurrent_requests: usize) -> Self {
        let endpoints = Vec::from_iter(endpoints);
        let channel = Channel::balance_list(endpoints.iter().cloned());
        Self {
            endpoints,
            channel: Mutex::new((0, channel)),
            request_semaphore: (max_concurrent_requests > 0).then_some(Semaphore::new(max_concurrent_requests)),
        }
    }

    /// Get a connection slot for an Endpoint, this contains a Channel which
    /// should be used once and any errors should be reported back to the
    /// on_error method to ensure that the Channel is re-connected on error.
    pub async fn get_connection(&self) -> (Connection<'_>, Channel) {
        let _permit = if let Some(semaphore) = &self.request_semaphore {
            Some(semaphore.acquire().await)
        } else {
            None
        };
        let channel_lock = self.channel.lock();
        (
            Connection {
                channel_id: channel_lock.0,
                parent: self,
                _permit,
            },
            channel_lock.1.clone(),
        )
    }
}

/// An instance of this is obtained for every communication with the gGRPC
/// service.  This handles the permit for limiting concurrency, and also
/// re-connecting the underlying channel on error.  It depends on users
/// reporting all errors.
pub struct Connection<'a> {
    channel_id: usize,
    parent: &'a ConnectionManager,
    _permit: Option<SemaphoreGuard<'a>>,
}

impl<'a> Connection<'a> {
    pub fn on_error(self, err: &Error) {
        // Usually Tonic reconnects on upstream errors (like Unavailable) but
        // if there are protocol errors (such as GoAway) then it will not
        // attempt to re-connect, and therefore we are forced to manually do
        // that.
        if err.code != Code::Internal {
            return;
        }
        // Create a new channel for future requests to use upon a new request
        // to ConnectionManager::get_connection().  In order to ensure we only
        // do this for the first error on a cloned Channel we check the ID
        // matches the current ID when we get the lock.
        let mut channel_lock = self.parent.channel.lock();
        if channel_lock.0 != self.channel_id {
            // The connection was already re-established by another user getting
            // and error on a clone of this Channel, so don't make another one.
            return;
        }
        // Create a new channel with a unique ID to track if it gets an error.
        // This new Channel will be used when a new request comes into
        // ConnectionManager::get_connection() as this request has been and gone
        // with an error now and it's up to the user whether they retry by
        // getting a new connection.
        channel_lock.0 += 1;
        channel_lock.1 = Channel::balance_list(self.parent.endpoints.iter().cloned());
    }
}
