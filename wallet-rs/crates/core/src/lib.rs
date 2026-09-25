//! Core of the native Chia wallet. No UI code lives here, so everything that
//! touches money, keys or the network can be tested headlessly.
//!
//! No floating point anywhere: amounts are integer mojos end to end.
#![deny(clippy::float_arithmetic)]

pub mod address;
pub mod api;
pub mod bip39;
pub mod client;
pub mod config;
pub mod demo;
pub mod protocol;
pub mod secret;
pub mod units;

pub use api::{KeyOrigin, WalletApi};
pub use client::{DaemonClient, RpcError, Transport};
pub use demo::DemoTransport;
