//! MySQL driver on mysql_async (the copy in `vendor/`, see its README).
//!
//! * One connection per session, by role (`SessionRole`): a *query* session per tab and one
//!   *meta* session per profile, as the PostgreSQL driver has them.
//! * Every connection goes over a stream the driver opens itself (`route`): a TCP connection
//!   to the server, or a channel of the profile's SSH tunnel; mysql_async speaks its protocol
//!   over it (`Conn::connect_with_stream`, the vendored copy's addition). No local listener is
//!   ever opened for a tunnel.
//! * The client never asks for multi-statements (a text the server reads as several statements
//!   fails as a whole) nor for `LOAD DATA LOCAL` (a server could read any file of the client),
//!   and keeps the session's character set UTF-8 (`utf8mb4`): the MySQL risk classifier reads
//!   the text as a UTF-8 session does.
//! * A full `caching_sha2_password` login (the account is not in the server's cache) sends the
//!   password encrypted with the server's RSA public key: the profile's key file, else the key
//!   the server sends when asked. A direct connection to another machine never asks (the key
//!   would come over the same unencrypted connection) unless the profile allows it, and fails
//!   before anything about the password is sent.
//! * The server says what changes in the session in its OK packets (session state tracking,
//!   asked for at connect: `session`): the sql mode the text is read in, read-only, the current
//!   database.
//! * MySQL 8.0 and newer; MariaDB 10.6 and newer as a best effort. Older servers are refused when
//!   the session opens, with their version.

mod connect;
mod link;
mod query;
mod route;
mod server_key;
mod session;
mod values;
mod wire;

pub use connect::MyDriver;
