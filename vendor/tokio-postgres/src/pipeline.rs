// datarig: this whole file is an addition (see vendor/README.md).
//
// Requests whose extended-protocol messages go out in one write and end with one `Sync`, so
// they cost one round trip where the requests of upstream's API cost one each:
//
// * `Transaction::bind_first_page`: [BEGIN] + Bind + Describe (portal) + Execute(max_rows),
//   the first rows of a portal together with the portal's current description (the `BEGIN`
//   may be several statements: `BEGIN READ ONLY` and `SET LOCAL search_path …`, or `SET
//   TRANSACTION READ ONLY` in the caller's block);
// * `Client::execute_pipelined`: [before] + Parse + Describe (statement) + Bind +
//   Execute(max_rows) of the unnamed statement + [after], for a statement expected to return
//   no rows (`before`/`after`: e.g. `BEGIN READ ONLY` and `COMMIT`);
// * `Client::prepare_wrapped`: [before] + Parse + Describe (statement) + [after], a named
//   statement prepared in a transaction of the caller's (e.g. `BEGIN` and `SET LOCAL
//   search_path …`, then `COMMIT`);
// * `Client::prepare_unnamed_wrapped`: the same for the unnamed statement (e.g. after `BEGIN`,
//   without `COMMIT`: a statement that lives only in the transaction it is used in).
//
// A pipelined `BEGIN` is sent as Parse/Bind/Execute of the unnamed statement and portal (a
// simple `Query` would end with its own `ReadyForQuery`, and each request here is answered by
// exactly one). Executed after the implicit transaction of the extended protocol started, it
// turns that transaction into a block, so the portal bound after it outlives the `Sync`.
//
// Every response is read up to its `ReadyForQuery` before types are looked up: looking up an
// unknown type is a request of its own, whose answer comes after this one's.

use crate::client::InnerClient;
use crate::codec::FrontendMessage;
use crate::connection::RequestMessages;
use crate::prepare::get_type;
use crate::query::extract_row_affected;
use crate::statement::Column;
use crate::types::Type;
use crate::{Error, Portal, Row, Statement};
use bytes::BytesMut;
use fallible_iterator::FallibleIterator;
use postgres_protocol::message::backend::{DataRowBody, Message, RowDescriptionBody};
use postgres_protocol::message::frontend;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

/// The first rows of a portal, read in the round trip that bound it.
pub struct FirstPage {
    /// The portal, open while its transaction lasts (unless `complete`).
    pub portal: Portal,
    /// The portal's columns as the server describes them at bind time. A statement prepared
    /// earlier keeps the names and table columns it was prepared with; these are current.
    pub columns: Vec<Column>,
    /// Up to `max_rows` rows, decoded with the statement's column types.
    pub rows: Vec<Row>,
    /// The portal ran to its end (`CommandComplete`) rather than being suspended.
    pub complete: bool,
    /// The rows affected, from the command tag, when it ran to its end.
    pub rows_affected: Option<u64>,
}

/// What [`execute_pipelined`] got back.
pub struct Executed {
    /// The statement's description (`columns()`), when it returned rows after all.
    pub statement: Option<Statement>,
    /// Up to `max_rows` rows (text format), when it returned rows.
    pub rows: Vec<Row>,
    /// The rows affected, from the command tag (`None` for an empty query).
    pub rows_affected: Option<u64>,
    /// The portal ran to its end (`false`: rows were left behind; the implicit transaction
    /// ended with the `Sync`, so they are gone).
    pub complete: bool,
}

/// Parse/Bind/Execute of `sql` (a statement without parameters or rows, e.g. `BEGIN`) as the
/// unnamed statement and portal: answered with ParseComplete, BindComplete, CommandComplete.
fn simple(sql: &str, buf: &mut BytesMut) -> Result<(), Error> {
    frontend::parse("", sql, std::iter::empty(), buf).map_err(Error::encode)?;
    frontend::bind(
        "",
        "",
        std::iter::empty::<i16>(),
        std::iter::empty::<()>(),
        |_: (), _: &mut BytesMut| Ok::<_, Box<dyn std::error::Error + Sync + Send>>(postgres_protocol::IsNull::No),
        std::iter::empty::<i16>(),
        buf,
    )
    .map_err(|_| Error::unexpected_message())?;
    frontend::execute("", 0, buf).map_err(Error::encode)
}

/// Every message of the response to one request, up to its `ReadyForQuery`. The first error
/// comes back as it is (the rest of the response is dropped with the stream).
async fn read_all(client: &InnerClient, buf: bytes::Bytes) -> Result<Vec<Message>, Error> {
    let mut responses = client.send(RequestMessages::Single(FrontendMessage::Raw(buf)))?;
    let mut out = Vec::new();
    loop {
        match responses.next().await? {
            Message::ReadyForQuery(_) => return Ok(out),
            m => out.push(m),
        }
    }
}

/// The columns of a RowDescription, with the types of `known` (the statement's) where the
/// type is the same, else looked up.
async fn columns_of(
    client: &Arc<InnerClient>,
    body: &RowDescriptionBody,
    known: &[Column],
) -> Result<Vec<Column>, Error> {
    let mut fields = body.fields();
    let mut out = Vec::new();
    while let Some(f) = fields.next().map_err(Error::parse)? {
        let r#type = match known.get(out.len()).map(|c| &c.r#type).filter(|t| t.oid() == f.type_oid()) {
            Some(t) => t.clone(),
            None => get_type(client, f.type_oid()).await?,
        };
        out.push(Column {
            name: f.name().to_string(),
            table_oid: Some(f.table_oid()).filter(|n| *n != 0),
            column_id: Some(f.column_id()).filter(|n| *n != 0),
            type_modifier: f.type_modifier(),
            r#type,
        });
    }
    Ok(out)
}

/// The encoded request of [`bind_first_page`](crate::Transaction::bind_first_page): nothing is
/// sent yet, so a statement that cannot be bound (wrong number of parameters) sends nothing.
pub(crate) struct FirstPageRequest {
    buf: bytes::Bytes,
    portal: String,
    /// How many statements went out before the Bind.
    begins: usize,
}

/// Skips the three answers to a statement sent with [`simple`].
fn skip_simple(it: &mut impl Iterator<Item = Message>) -> Result<(), Error> {
    for _ in 0..3 {
        match it.next() {
            Some(Message::ParseComplete | Message::BindComplete | Message::CommandComplete(_)) => {}
            _ => return Err(Error::unexpected_message()),
        }
    }
    Ok(())
}

pub(crate) fn encode_first_page(
    client: &InnerClient,
    statement: &Statement,
    result_formats: &[i16],
    max_rows: i32,
    begin: &[String],
) -> Result<FirstPageRequest, Error> {
    let portal = format!("q{}", NEXT_ID.fetch_add(1, Ordering::SeqCst));
    let buf = client.with_buf(|buf| {
        for b in begin {
            simple(b, buf)?;
        }
        crate::query::encode_bind_with_formats(statement, std::iter::empty::<&str>(), &portal, result_formats, buf)?;
        frontend::describe(b'P', &portal, buf).map_err(Error::encode)?;
        frontend::execute(&portal, max_rows, buf).map_err(Error::encode)?;
        frontend::sync(buf);
        Ok::<_, Error>(buf.split().freeze())
    })?;
    Ok(FirstPageRequest { buf, portal, begins: begin.len() })
}

pub(crate) async fn read_first_page(
    client: &Arc<InnerClient>,
    statement: &Statement,
    request: FirstPageRequest,
) -> Result<FirstPage, Error> {
    let FirstPageRequest { buf, portal: name, begins } = request;
    let messages = read_all(client, buf).await?;
    let mut it = messages.into_iter();
    for _ in 0..begins {
        skip_simple(&mut it)?;
    }
    match it.next() {
        Some(Message::BindComplete) => {}
        _ => return Err(Error::unexpected_message()),
    }
    // Closed on the server when dropped (or with its transaction).
    let portal = Portal::new(client, name, statement.clone());
    let columns = match it.next() {
        Some(Message::RowDescription(body)) => columns_of(client, &body, statement.columns()).await?,
        Some(Message::NoData) => Vec::new(),
        _ => return Err(Error::unexpected_message()),
    };
    let mut rows = Vec::new();
    let mut complete = false;
    let mut rows_affected = None;
    for m in it {
        match m {
            Message::DataRow(body) => rows.push(Row::new(statement.clone(), body)?),
            Message::CommandComplete(body) => {
                rows_affected = Some(extract_row_affected(&body)?);
                complete = true;
            }
            Message::EmptyQueryResponse => complete = true,
            Message::PortalSuspended => complete = false,
            _ => return Err(Error::unexpected_message()),
        }
    }
    Ok(FirstPage { portal, columns, rows, complete, rows_affected })
}

pub(crate) async fn execute_pipelined(
    client: &Arc<InnerClient>,
    query: &str,
    max_rows: i32,
    before: &[String],
    after: Option<&str>,
) -> Result<Executed, Error> {
    let buf = client.with_buf(|buf| {
        for b in before {
            simple(b, buf)?;
        }
        frontend::parse("", query, std::iter::empty(), buf).map_err(Error::encode)?;
        frontend::describe(b'S', "", buf).map_err(Error::encode)?;
        frontend::bind(
            "",
            "",
            std::iter::empty::<i16>(),
            std::iter::empty::<()>(),
            |_: (), _: &mut BytesMut| Ok::<_, Box<dyn std::error::Error + Sync + Send>>(postgres_protocol::IsNull::No),
            // No result format codes: every column in text.
            std::iter::empty::<i16>(),
            buf,
        )
        .map_err(|_| Error::unexpected_message())?;
        frontend::execute("", max_rows, buf).map_err(Error::encode)?;
        if let Some(after) = after {
            simple(after, buf)?;
        }
        frontend::sync(buf);
        Ok::<_, Error>(buf.split().freeze())
    })?;
    let mut messages = read_all(client, buf).await?;
    if after.is_some() {
        let tail = messages.split_off(messages.len().saturating_sub(3));
        skip_simple(&mut tail.into_iter())?;
    }
    let mut messages = messages.into_iter();
    for _ in before {
        skip_simple(&mut messages)?;
    }
    let mut description = None;
    let mut bodies: Vec<DataRowBody> = Vec::new();
    let mut rows_affected = None;
    let mut complete = true;
    for m in messages {
        match m {
            Message::ParseComplete | Message::BindComplete | Message::ParameterDescription(_) | Message::NoData => {}
            Message::RowDescription(body) => description = Some(body),
            Message::DataRow(body) => bodies.push(body),
            Message::CommandComplete(body) => rows_affected = Some(extract_row_affected(&body)?),
            Message::EmptyQueryResponse => {}
            Message::PortalSuspended => complete = false,
            _ => return Err(Error::unexpected_message()),
        }
    }
    let Some(body) = description else {
        return Ok(Executed { statement: None, rows: Vec::new(), rows_affected, complete });
    };
    let statement = Statement::unnamed(Vec::<Type>::new(), columns_of(client, &body, &[]).await?);
    let rows = bodies.into_iter().map(|b| Row::new(statement.clone(), b)).collect::<Result<Vec<_>, _>>()?;
    Ok(Executed { statement: Some(statement), rows, rows_affected, complete })
}

pub(crate) async fn prepare_wrapped(
    client: &Arc<InnerClient>,
    query: &str,
    before: &[String],
    after: Option<&str>,
    named: bool,
) -> Result<Statement, Error> {
    let name = if named { crate::prepare::next_name() } else { String::new() };
    let buf = client.with_buf(|buf| {
        for b in before {
            simple(b, buf)?;
        }
        frontend::parse(&name, query, std::iter::empty(), buf).map_err(Error::encode)?;
        frontend::describe(b'S', &name, buf).map_err(Error::encode)?;
        if let Some(after) = after {
            simple(after, buf)?;
        }
        frontend::sync(buf);
        Ok::<_, Error>(buf.split().freeze())
    })?;
    let mut messages = read_all(client, buf).await?;
    if after.is_some() {
        let tail = messages.split_off(messages.len().saturating_sub(3));
        skip_simple(&mut tail.into_iter())?;
    }
    let mut it = messages.into_iter();
    for _ in before {
        skip_simple(&mut it)?;
    }
    match it.next() {
        Some(Message::ParseComplete) => {}
        _ => return Err(Error::unexpected_message()),
    }
    let parameters = match it.next() {
        Some(Message::ParameterDescription(body)) => body,
        _ => return Err(Error::unexpected_message()),
    };
    let columns = match it.next() {
        Some(Message::RowDescription(body)) => columns_of(client, &body, &[]).await?,
        Some(Message::NoData) => Vec::new(),
        _ => return Err(Error::unexpected_message()),
    };
    let mut types = Vec::new();
    let mut oids = parameters.parameters();
    while let Some(oid) = oids.next().map_err(Error::parse)? {
        types.push(get_type(client, oid).await?);
    }
    if named {
        Ok(Statement::new(client, name, types, columns))
    } else {
        Ok(Statement::unnamed(types, columns))
    }
}
