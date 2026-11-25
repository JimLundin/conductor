use crate::prelude::*;
use crate::{Invocation, Provider, ProviderInfo, SignIn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub file: String,
    pub contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reported {
    Status,
    File(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub provider: Provider,
    pub info: ProviderInfo,
    pub config: Config,
    pub instructions_file: String,
    pub version: Invocation,
    pub status: Invocation,
    pub account: Reported,
    pub sign_in: SignIn,
}
