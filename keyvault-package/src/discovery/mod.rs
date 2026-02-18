//! Discovery poller — daily (and on-startup) scan of all providers.
//!
//! For every registered key, probes the provider to discover:
//! - Available models (new, deprecated)
//! - Key health (valid, quota remaining, reset time)
//! - Rate limit state
//! - Error details (why it failed, when it resets)

pub mod poller;
