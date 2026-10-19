#![allow(unused)]
#![allow(private_interfaces)]
#![allow(private_bounds)]

#[macro_use]
extern crate async_trait;
#[macro_use]
extern crate atomic_enum;

pub mod identity;
mod log;
#[cfg(feature = "matrix")]
pub mod matrix;
pub mod settings;
pub mod sink;
pub mod source;
#[cfg(feature = "ssh")]
mod ssh_session;
mod state;
