// #![deny(missing_docs)]

//! A lending program for the Solana blockchain.

// Export current sdk types for downstream users building with a different sdk version
pub use solana_program;

pub mod entrypoint;
pub mod error;
pub mod instruction;
pub mod math;
pub mod processor;
pub mod pyth;
pub mod state;
pub mod unpack_util;


solana_program::declare_id!("TokenLending1111111111111111111111111111111");
