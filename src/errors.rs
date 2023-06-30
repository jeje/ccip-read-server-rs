//use ethers_providers::{Middleware, MiddlewareError};
use thiserror::Error;

/// Handle CCIP-Read middleware specific errors.
#[derive(Error, Debug)]
pub enum CCIPReadMiddlewareError /*<M: Middleware>*/ {
    #[error("Unknown function")]
    UnknownFunction(#[from] ethers_core::abi::Error),

    #[error("Parsing error")]
    Parsing(#[from] serde_json::Error),

    #[error("Abi error")]
    Abi(#[from] ethers_core::abi::AbiError),

    #[error("Parse bytes error")]
    ParseBytes(#[from] ethers_core::types::ParseBytesError),
}
