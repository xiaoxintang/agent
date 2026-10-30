//! Optional Web management plane; each module owns one subsystem.
mod assignments;
mod auth;
mod contract;
pub mod db;
mod events;
mod management;
mod operations;
mod proxy;
mod server;
mod state;
#[cfg(test)]
mod tests;
mod types;
mod worker;

pub use contract::contract;
pub use operations::{endpoint, mutating};
pub use server::{router, serve};
pub use state::Web;
pub use types::{Effect, Invoke, Task, segment};
