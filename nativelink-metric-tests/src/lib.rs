// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use std::fmt::Debug;
use std::marker::PhantomData;

use nativelink_metric::MetricsComponent;

#[derive(MetricsComponent)]
pub struct SharedContext {
    #[metric(help = "Number of active drop spawns")]
    pub active_drop_spawns: u64,

    #[metric(help = "Path to the configured temp path")]
    temp_path: String,

    #[metric(help = "Path to the configured content path")]
    content_path: String,

    #[metric(group = "foo")]
    test: Foo<'static, String>,
    // #[metric]
    // root_field: FooBar,
}

#[derive(MetricsComponent)]
struct Foo<'a, T: Debug + Send + Sync> {
    #[metric(handler = ToString::to_string)]
    foo: u64,
    _bar: &'a PhantomData<T>,
}

// #[derive(MetricsComponent)]
// struct FooBar(#[metric] u64);
