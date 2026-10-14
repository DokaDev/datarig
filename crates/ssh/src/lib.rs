//! SSH tunnels: a bastion reached with a key file,
//! a password, keyboard-interactive answers or the ssh-agent, whose `direct-tcpip` channels
//! are the byte streams a driver connects through (`datarig_core::transport::Dialer`).
//!
//! The only crate that names russh. It opens a tunnel when the app asks and never on its own;
//! host keys are checked against the user's OpenSSH known_hosts (read only) and datarig's own
//! file (`known_hosts`); every question for the user (a host key, a passphrase, a
//! keyboard-interactive prompt) goes to the app through [`tunnel::Asker`].

pub mod keys;
pub mod known_hosts;
pub mod tunnel;

pub use known_hosts::KnownHosts;
pub use tunnel::{Asker, Auth, Env, Hop, Options, SshError, Tunnel};
