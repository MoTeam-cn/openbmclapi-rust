//! Upstream resilience primitives shared by the WebDAV-based backends.
//!
//! AList/OpenList falls over when it is pushed harder than it can serve, and the
//! Node agent used to die with it. These primitives let the agent shed load
//! instead of piling on: a circuit breaker stops hammering an upstream that is
//! already failing, an adaptive limiter discovers how much concurrency the
//! upstream can actually take, and the retry policy backs off on the statuses
//! that mean "come back later".

mod breaker;
mod client;
mod limiter;
mod retry;

pub(crate) use client::{ResilienceConfig, ResilientClient};
