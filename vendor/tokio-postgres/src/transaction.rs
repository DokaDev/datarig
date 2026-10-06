#[cfg(feature = "runtime")]
use crate::Socket;
use crate::copy_out::CopyOutStream;
use crate::query::RowStream;
#[cfg(feature = "runtime")]
use crate::tls::MakeTlsConnect;
use crate::tls::TlsConnect;
use crate::types::{BorrowToSql, ToSql, Type};
use crate::{
    CancelToken, Client, CopyInSink, Error, Portal, Row, SimpleQueryMessage, Statement,
    ToStatement, bind, query, slice_iter,
};
use bytes::Buf;
use futures_util::TryStreamExt;
use tokio::io::{AsyncRead, AsyncWrite};

/// A representation of a PostgreSQL database transaction.
///
/// Transactions will implicitly roll back when dropped. Use the `commit` method to commit the changes made in the
/// transaction. Transactions can be nested, with inner transactions implemented via safepoints.
pub struct Transaction<'a> {
    client: &'a mut Client,
    savepoint: Option<Savepoint>,
    done: bool,
    // datarig: `BEGIN` has not been sent yet: it goes out with the first request made through
    // the transaction (`Client::transaction_pipelined`, `bind_first_page`).
    begin: std::sync::atomic::AtomicBool,
    // datarig: the statements sent as that `BEGIN` (`BEGIN READ ONLY`, then `SET LOCAL`s, or
    // for a handle on the caller's block statements to run first in it, e.g. `SET TRANSACTION
    // READ ONLY`).
    begin_sql: Vec<String>,
}

/// A representation of a PostgreSQL database savepoint.
struct Savepoint {
    name: String,
    depth: u32,
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        // datarig: nothing to roll back while `BEGIN` has not been sent.
        if self.done || *self.begin.get_mut() {
            return;
        }

        let name = self.savepoint.as_ref().map(|sp| sp.name.as_str());
        self.client.__private_api_rollback(name);
    }
}

impl<'a> Transaction<'a> {
    pub(crate) fn new(client: &'a mut Client) -> Transaction<'a> {
        Transaction {
            client,
            savepoint: None,
            done: false,
            begin: std::sync::atomic::AtomicBool::new(false),
            begin_sql: Vec::new(),
        }
    }

    // datarig: a transaction whose `BEGIN` (`begin`, e.g. `BEGIN READ ONLY` and the statements
    // to run first in it) is sent with its first request, or a handle on a block the caller
    // opened itself, which it never ends (`done`), with statements (`begin`) to send first in
    // it, if any.
    pub(crate) fn new_datarig(client: &'a mut Client, begin: Vec<String>, done: bool) -> Transaction<'a> {
        Transaction {
            client,
            savepoint: None,
            done,
            begin: std::sync::atomic::AtomicBool::new(!begin.is_empty()),
            begin_sql: begin,
        }
    }

    /// Consumes the transaction, committing all changes made within it.
    pub async fn commit(mut self) -> Result<(), Error> {
        // datarig: a transaction that never started (or a caller's block) has nothing to end.
        if self.done || *self.begin.get_mut() {
            self.done = true;
            return Ok(());
        }
        self.done = true;
        let query = if let Some(sp) = self.savepoint.as_ref() {
            format!("RELEASE {}", sp.name)
        } else {
            "COMMIT".to_string()
        };
        self.client.batch_execute(&query).await
    }

    /// Rolls the transaction back, discarding all changes made within it.
    ///
    /// This is equivalent to `Transaction`'s `Drop` implementation, but provides any error encountered to the caller.
    pub async fn rollback(mut self) -> Result<(), Error> {
        // datarig: a transaction that never started (or a caller's block) has nothing to end.
        if self.done || *self.begin.get_mut() {
            self.done = true;
            return Ok(());
        }
        self.done = true;
        let query = if let Some(sp) = self.savepoint.as_ref() {
            format!("ROLLBACK TO {}", sp.name)
        } else {
            "ROLLBACK".to_string()
        };
        self.client.batch_execute(&query).await
    }

    /// Like `Client::prepare`.
    pub async fn prepare(&self, query: &str) -> Result<Statement, Error> {
        self.client.prepare(query).await
    }

    /// Like `Client::prepare_typed`.
    pub async fn prepare_typed(
        &self,
        query: &str,
        parameter_types: &[Type],
    ) -> Result<Statement, Error> {
        self.client.prepare_typed(query, parameter_types).await
    }

    /// Like `Client::query`.
    pub async fn query<T>(
        &self,
        statement: &T,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<Row>, Error>
    where
        T: ?Sized + ToStatement,
    {
        self.client.query(statement, params).await
    }

    /// Like `Client::query_one`.
    pub async fn query_one<T>(
        &self,
        statement: &T,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Row, Error>
    where
        T: ?Sized + ToStatement,
    {
        self.client.query_one(statement, params).await
    }

    /// Like `Client::query_opt`.
    pub async fn query_opt<T>(
        &self,
        statement: &T,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Option<Row>, Error>
    where
        T: ?Sized + ToStatement,
    {
        self.client.query_opt(statement, params).await
    }

    /// Like `Client::query_raw`.
    pub async fn query_raw<T, P, I>(&self, statement: &T, params: I) -> Result<RowStream, Error>
    where
        T: ?Sized + ToStatement,
        P: BorrowToSql,
        I: IntoIterator<Item = P>,
        I::IntoIter: ExactSizeIterator,
    {
        self.client.query_raw(statement, params).await
    }

    /// Like `Client::query_typed`.
    pub async fn query_typed(
        &self,
        statement: &str,
        params: &[(&(dyn ToSql + Sync), Type)],
    ) -> Result<Vec<Row>, Error> {
        self.client.query_typed(statement, params).await
    }

    /// Like `Client::query_typed_one`.
    pub async fn query_typed_one(
        &self,
        statement: &str,
        params: &[(&(dyn ToSql + Sync), Type)],
    ) -> Result<Row, Error> {
        self.client.query_typed_one(statement, params).await
    }

    /// Like `Client::query_typed_opt`.
    pub async fn query_typed_opt(
        &self,
        statement: &str,
        params: &[(&(dyn ToSql + Sync), Type)],
    ) -> Result<Option<Row>, Error> {
        self.client.query_typed_opt(statement, params).await
    }

    /// Like `Client::query_typed_raw`.
    pub async fn query_typed_raw<P, I>(&self, query: &str, params: I) -> Result<RowStream, Error>
    where
        P: BorrowToSql,
        I: IntoIterator<Item = (P, Type)>,
    {
        self.client.query_typed_raw(query, params).await
    }

    /// Like `Client::execute`.
    pub async fn execute<T>(
        &self,
        statement: &T,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<u64, Error>
    where
        T: ?Sized + ToStatement,
    {
        self.client.execute(statement, params).await
    }

    /// Like `Client::execute_typed`.
    pub async fn execute_typed(
        &self,
        statement: &str,
        params: &[(&(dyn ToSql + Sync), Type)],
    ) -> Result<u64, Error> {
        self.client.execute_typed(statement, params).await
    }

    /// Like `Client::execute_iter`.
    pub async fn execute_raw<P, I, T>(&self, statement: &T, params: I) -> Result<u64, Error>
    where
        T: ?Sized + ToStatement,
        P: BorrowToSql,
        I: IntoIterator<Item = P>,
        I::IntoIter: ExactSizeIterator,
    {
        self.client.execute_raw(statement, params).await
    }

    /// Binds a statement to a set of parameters, creating a `Portal` which can be incrementally queried.
    ///
    /// Portals only last for the duration of the transaction in which they are created, and can only be used on the
    /// connection that created them.
    ///
    /// # Panics
    ///
    /// Panics if the number of parameters provided does not match the number expected.
    pub async fn bind<T>(
        &self,
        statement: &T,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Portal, Error>
    where
        T: ?Sized + ToStatement,
    {
        self.bind_raw(statement, slice_iter(params)).await
    }

    /// A maximally flexible version of [`bind`].
    ///
    /// [`bind`]: #method.bind
    pub async fn bind_raw<P, T, I>(&self, statement: &T, params: I) -> Result<Portal, Error>
    where
        T: ?Sized + ToStatement,
        P: BorrowToSql,
        I: IntoIterator<Item = P>,
        I::IntoIter: ExactSizeIterator,
    {
        let statement = statement
            .__convert()
            .into_statement(self.client.inner())
            .await?;
        bind::bind(self.client.inner(), statement, params).await
    }

    /// datarig: like [`bind`], with the result format of each column of the statement (`0`
    /// text, `1` binary; a single code applies to every column). A text column's `Row` values
    /// are PostgreSQL's text output; read them as raw bytes, not with a typed `FromSql`.
    ///
    /// [`bind`]: #method.bind
    pub async fn bind_with_formats<T>(
        &self,
        statement: &T,
        params: &[&(dyn ToSql + Sync)],
        result_formats: &[i16],
    ) -> Result<Portal, Error>
    where
        T: ?Sized + ToStatement,
    {
        let statement = statement
            .__convert()
            .into_statement(self.client.inner())
            .await?;
        bind::bind_with_formats(
            self.client.inner(),
            statement,
            slice_iter(params),
            result_formats,
        )
        .await
    }

    /// datarig: binds `statement` (no parameters) to a new portal with `result_formats` (as
    /// [`bind_with_formats`]) and fetches its first `max_rows` rows and its current description,
    /// in one round trip: Bind, Describe, Execute and Sync go out in one write, after the
    /// transaction's `BEGIN` when it has not been sent yet.
    ///
    /// [`bind_with_formats`]: #method.bind_with_formats
    pub async fn bind_first_page(
        &self,
        statement: &Statement,
        result_formats: &[i16],
        max_rows: i32,
    ) -> Result<crate::pipeline::FirstPage, Error> {
        let begin: &[String] =
            if self.begin.load(std::sync::atomic::Ordering::SeqCst) { &self.begin_sql } else { &[] };
        let request = crate::pipeline::encode_first_page(self.client.inner(), statement, result_formats, max_rows, begin)?;
        // Encoded: `BEGIN` goes out with it, whatever the server answers.
        self.begin.store(false, std::sync::atomic::Ordering::SeqCst);
        crate::pipeline::read_first_page(self.client.inner(), statement, request).await
    }

    /// datarig: `bind_first_page`, then `end` (`COMMIT` or `ROLLBACK`, Parse/Bind/Execute of
    /// the unnamed statement) after the Execute, in the same write: the transaction ends in
    /// the round trip that read the first rows, and the portal with it, whether it ran to its end
    /// or not. Answered once `end` succeeded; the transaction is over then (`commit`, `rollback`
    /// and dropping it send nothing more). When the statement or `end` fails the transaction is
    /// left as after a failed `bind_first_page` (aborted, or ended by a failed `COMMIT`): roll it
    /// back (a `ROLLBACK` with no transaction open only warns).
    pub async fn bind_first_page_then(
        &mut self,
        statement: &Statement,
        result_formats: &[i16],
        max_rows: i32,
        end: &str,
    ) -> Result<crate::pipeline::FirstPage, Error> {
        let begin: &[String] =
            if self.begin.load(std::sync::atomic::Ordering::SeqCst) { &self.begin_sql } else { &[] };
        let request = crate::pipeline::encode_first_page_then(
            self.client.inner(),
            statement,
            result_formats,
            max_rows,
            begin,
            Some(end),
        )?;
        self.begin.store(false, std::sync::atomic::Ordering::SeqCst);
        let page = crate::pipeline::read_first_page(self.client.inner(), statement, request).await?;
        self.done = true;
        Ok(page)
    }

    /// Continues execution of a portal, returning a stream of the resulting rows.
    ///
    /// Unlike `query`, portals can be incrementally evaluated by limiting the number of rows returned in each call to
    /// `query_portal`. If the requested number is negative or 0, all rows will be returned.
    pub async fn query_portal(&self, portal: &Portal, max_rows: i32) -> Result<Vec<Row>, Error> {
        self.query_portal_raw(portal, max_rows)
            .await?
            .try_collect()
            .await
    }

    /// The maximally flexible version of [`query_portal`].
    ///
    /// [`query_portal`]: #method.query_portal
    pub async fn query_portal_raw(
        &self,
        portal: &Portal,
        max_rows: i32,
    ) -> Result<RowStream, Error> {
        query::query_portal(self.client.inner(), portal, max_rows).await
    }

    /// Like `Client::copy_in`.
    pub async fn copy_in<T, U>(&self, statement: &T) -> Result<CopyInSink<U>, Error>
    where
        T: ?Sized + ToStatement,
        U: Buf + 'static + Send,
    {
        self.client.copy_in(statement).await
    }

    /// Like `Client::copy_out`.
    pub async fn copy_out<T>(&self, statement: &T) -> Result<CopyOutStream, Error>
    where
        T: ?Sized + ToStatement,
    {
        self.client.copy_out(statement).await
    }

    /// Like `Client::simple_query`.
    pub async fn simple_query(&self, query: &str) -> Result<Vec<SimpleQueryMessage>, Error> {
        self.client.simple_query(query).await
    }

    /// Like `Client::batch_execute`.
    pub async fn batch_execute(&self, query: &str) -> Result<(), Error> {
        self.client.batch_execute(query).await
    }

    /// Like `Client::cancel_token`.
    pub fn cancel_token(&self) -> CancelToken {
        self.client.cancel_token()
    }

    /// Like `Client::cancel_query`.
    #[cfg(feature = "runtime")]
    #[deprecated(since = "0.6.0", note = "use Transaction::cancel_token() instead")]
    pub async fn cancel_query<T>(&self, tls: T) -> Result<(), Error>
    where
        T: MakeTlsConnect<Socket>,
    {
        #[allow(deprecated)]
        self.client.cancel_query(tls).await
    }

    /// Like `Client::cancel_query_raw`.
    #[deprecated(since = "0.6.0", note = "use Transaction::cancel_token() instead")]
    pub async fn cancel_query_raw<S, T>(&self, stream: S, tls: T) -> Result<(), Error>
    where
        S: AsyncRead + AsyncWrite + Unpin,
        T: TlsConnect<S>,
    {
        #[allow(deprecated)]
        self.client.cancel_query_raw(stream, tls).await
    }

    /// Like `Client::transaction`, but creates a nested transaction via a savepoint.
    pub async fn transaction(&mut self) -> Result<Transaction<'_>, Error> {
        self._savepoint(None).await
    }

    /// Like `Client::transaction`, but creates a nested transaction via a savepoint with the specified name.
    pub async fn savepoint<I>(&mut self, name: I) -> Result<Transaction<'_>, Error>
    where
        I: Into<String>,
    {
        self._savepoint(Some(name.into())).await
    }

    async fn _savepoint(&mut self, name: Option<String>) -> Result<Transaction<'_>, Error> {
        let depth = self.savepoint.as_ref().map_or(0, |sp| sp.depth) + 1;
        let name = name.unwrap_or_else(|| format!("sp_{depth}"));
        let query = format!("SAVEPOINT {name}");
        self.batch_execute(&query).await?;

        Ok(Transaction {
            client: self.client,
            savepoint: Some(Savepoint { name, depth }),
            done: false,
            // datarig: the savepoint was sent above.
            begin: std::sync::atomic::AtomicBool::new(false),
            begin_sql: Vec::new(),
        })
    }

    /// Returns a reference to the underlying `Client`.
    pub fn client(&self) -> &Client {
        self.client
    }
}
