//! Multiplexer: headless PTY session manager with attach/detach over Unix
//! sockets, remote SSH transport, and automatic reconnection.
//!
//! The mux server owns PTY processes and survives client disconnects.
//! Clients attach to named sessions, send input, and receive output.
//!
//! # Architecture
//!
//! ```text
//!  Winter (client) ←→ Unix socket ←→ Winter-mux (server) ←→ PTY
//!  Winter (client) ←→ SSH tunnel  ←→ remote Winter-mux ←→ PTY
//! ```

pub mod client;
pub mod persist;
pub mod protocol;
pub mod proxy;
pub mod remote;
pub mod resilience;
pub mod server;
pub mod session;

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    #[test]
    fn test_mux_crate_loads() {}
}
