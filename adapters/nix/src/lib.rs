//! Gate 1 Nix host adapter — probe, propose, and promotion primitives.

#![allow(
    clippy::must_use_candidate,
    clippy::expect_used,
    clippy::indexing_slicing
)]

pub mod adapter;
pub mod capture;
pub mod command_runner;
pub mod probe;
pub mod promotion;
pub mod propose;
pub mod protected;
