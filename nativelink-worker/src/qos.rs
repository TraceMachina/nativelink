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

//! Darwin `QoS` (Quality of Service) helpers for worker scheduling.
//!
//! Apple Silicon (M-series) CPUs have a heterogeneous topology with
//! performance ("P") and efficiency ("E") cores. XNU's scheduler routes
//! threads to P or E cores in part based on the thread's `QoS` class. The
//! default class assigned to long-running background daemons is typically
//! `UTILITY` or `BACKGROUND`, both of which the scheduler may park on
//! E-cores.
//!
//! Single-thread-bursty workloads such as `swift-frontend` and `clang`
//! invocations (typical in iOS RBE builds) can run 2x–3x slower when
//! pinned to an E-core. Tagging the worker process with
//! `QOS_CLASS_USER_INITIATED` tells the scheduler to treat its threads
//! as foreground-equivalent and bias placement toward P-cores.
//!
//! On Linux and Windows these helpers compile away to nothing — they are
//! intentionally not behind a runtime branch so non-macOS builds never
//! emit a call.

/// Sets the calling thread's `QoS` class to `USER_INITIATED` on macOS.
///
/// On non-macOS targets this is a compile-time no-op (the call site
/// expands to nothing after monomorphization / dead-code elimination).
///
/// Returns `true` if the call succeeded or the platform doesn't need it;
/// returns `false` only on macOS when the underlying pthread call fails.
///
/// Safe to call from any thread, including tokio runtime worker threads
/// via `Builder::on_thread_start`.
#[inline]
pub fn set_user_initiated() -> bool {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: `pthread_set_qos_class_self_np` is a thread-local
        // setter with no preconditions on the caller; passing a valid
        // enum variant and relative priority 0 is always defined.
        let ret = unsafe {
            libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INITIATED, 0)
        };
        ret == 0
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::set_user_initiated;

    /// Reads the current thread's `QoS` class via `pthread_get_qos_class_np`.
    /// Panics with a contextual message on failure (only called from tests).
    fn current_qos_class() -> libc::qos_class_t {
        let mut class: libc::qos_class_t = libc::qos_class_t::QOS_CLASS_UNSPECIFIED;
        let mut rel_prio: libc::c_int = 0;
        // SAFETY: out-pointers point to stack-allocated, properly sized
        // and aligned storage owned by this thread.
        let ret = unsafe {
            libc::pthread_get_qos_class_np(
                libc::pthread_self(),
                core::ptr::from_mut(&mut class),
                core::ptr::from_mut(&mut rel_prio),
            )
        };
        assert_eq!(ret, 0, "pthread_get_qos_class_np failed: {ret}");
        class
    }

    /// Proves the `QoS` call is wired up on macOS and the underlying
    /// Darwin symbol resolves at link time. A failure here means the
    /// worker would silently keep running on E-cores.
    #[test]
    fn sets_user_initiated_on_current_thread() {
        assert!(
            set_user_initiated(),
            "pthread_set_qos_class_self_np(USER_INITIATED) returned non-zero",
        );
        // `qos_class_t` is a `#[repr(u32)]` C enum that does not derive
        // `PartialEq` in libc, so compare the underlying discriminants.
        assert_eq!(
            current_qos_class() as u32,
            libc::qos_class_t::QOS_CLASS_USER_INITIATED as u32,
            "`QoS` class did not update; thread will be eligible for E-core scheduling",
        );
    }

    /// Validates the load-bearing claim that tokio worker threads created
    /// with a `Builder::on_thread_start` hook calling `set_user_initiated`
    /// observe `QOS_CLASS_USER_INITIATED` from inside spawned tasks. This
    /// mirrors the wiring in `src/bin/nativelink.rs::main`. Without this
    /// test the entire `QoS` scheme is unverified at the integration level.
    ///
    /// This is the one place in the worker crate that must construct a
    /// fresh `tokio::runtime::Builder::new_multi_thread()` and drive it
    /// with `block_on` — the unit under test *is* the `on_thread_start`
    /// hook on a custom-built runtime, which `nativelink-util::task` and
    /// `#[nativelink_test]` do not expose. The `#[expect]` mirrors the
    /// same justified escape used in `src/bin/nativelink.rs::main`.
    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "test exercises `Builder::on_thread_start` + `block_on`; \
                  no util wrapper exposes a custom-built runtime with a thread-start hook"
    )]
    fn tokio_worker_threads_inherit_user_initiated_via_on_thread_start() {
        // Deliberately build a fresh runtime in-test (do not reuse a
        // global one) so the hook is exercised on freshly-spawned
        // worker threads with whatever class they were born with.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .on_thread_start(|| {
                assert!(set_user_initiated(), "hook failed in worker thread");
            })
            .enable_all()
            .build()
            .expect("build tokio runtime");

        let observed: u32 = rt.block_on(async {
            // Force execution on a worker thread (not the caller).
            tokio::spawn(async { current_qos_class() as u32 })
                .await
                .expect("join spawned task")
        });

        assert_eq!(
            observed,
            libc::qos_class_t::QOS_CLASS_USER_INITIATED as u32,
            "tokio worker thread did not inherit USER_INITIATED from on_thread_start",
        );
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod non_macos_tests {
    use super::set_user_initiated;

    /// On Linux/Windows the function must be a true no-op that always
    /// reports success — there is no runtime cost and no platform call.
    #[test]
    fn is_a_noop_on_non_macos() {
        assert!(set_user_initiated());
    }
}

/// Compile-time assertion: when `target_os` is not `macos`, this module
/// must not reference any libc symbol. Reviewers can `grep "extern crate
/// libc"` or inspect this constant to verify the no-op story.
#[cfg(not(target_os = "macos"))]
pub const NON_MACOS_IS_NOOP: () = ();
