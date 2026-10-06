use crate::Statement;
use crate::client::InnerClient;
use crate::codec::FrontendMessage;
use crate::connection::RequestMessages;
use postgres_protocol::message::frontend;
use std::sync::{Arc, Weak};

struct Inner {
    client: Weak<InnerClient>,
    name: String,
    statement: Statement,
    // datarig: the portal ended with its transaction in the request that bound it (a pipelined
    // `COMMIT`): there is nothing to close, and a `Close` would be a request of its own.
    ended: bool,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        if let Some(client) = self.client.upgrade() {
            let buf = client.with_buf(|buf| {
                frontend::close(b'P', &self.name, buf).unwrap();
                frontend::sync(buf);
                buf.split().freeze()
            });
            let _ = client.send(RequestMessages::Single(FrontendMessage::Raw(buf)));
        }
    }
}

/// A portal.
///
/// Portals can only be used with the connection that created them, and only exist for the duration of the transaction
/// in which they were created.
#[derive(Clone)]
pub struct Portal(Arc<Inner>);

impl Portal {
    pub(crate) fn new(client: &Arc<InnerClient>, name: String, statement: Statement) -> Portal {
        Portal(Arc::new(Inner {
            client: Arc::downgrade(client),
            name,
            statement,
            ended: false,
        }))
    }

    // datarig: a portal that already ended with its transaction (see `Inner::ended`).
    pub(crate) fn ended(client: &Arc<InnerClient>, name: String, statement: Statement) -> Portal {
        Portal(Arc::new(Inner {
            client: Arc::downgrade(client),
            name,
            statement,
            ended: true,
        }))
    }

    pub(crate) fn name(&self) -> &str {
        &self.0.name
    }

    pub(crate) fn statement(&self) -> &Statement {
        &self.0.statement
    }
}
