# TLS

PostgreSQL and MySQL connections are encrypted with TLS 1.2 or 1.3 (rustls with `ring`) as
the profile's **SSL mode** says, in each database's own words. The profile form's Advanced
section has the mode, the **CA file**, the **client certificate** and its **key**. In the
config file they are `sslmode`, `sslrootcert`, `sslcert` and `sslkey`, and a PostgreSQL URL
takes the same four as parameters:

```text
postgres://me@db/app?sslmode=verify-full&sslrootcert=...
```

A MySQL URL takes no TLS parameters; use the form's fields.

## Modes

| PostgreSQL | MySQL | Encrypted | What is checked of the server's certificate |
|---|---|---|---|
| `disable` | `DISABLED` | never | nothing |
| `prefer` | `PREFERRED` | when the server offers TLS | nothing |
| `require` | `REQUIRED` | always | nothing; with a CA file, as `verify-ca` |
| `verify-ca` | `VERIFY_CA` | always | its chain, against the CA file (needed) |
| `verify-full` | `VERIFY_IDENTITY` | always | its chain, and that it is for the profile's host |

PostgreSQL's `allow` is not supported. MySQL's mode names are read in any case.

- `prefer` / `PREFERRED` goes on unencrypted only when the server says it has no TLS. A server
  that offers TLS and then fails the handshake is an error, never a plain connection.
- **The CA file is the only trust.** With one, only its certificates are trusted (a server's own
  self-signed certificate copied to your machine works too). Without one, `verify-full` /
  `VERIFY_IDENTITY` checks the certificate against the certificates your system trusts: the
  macOS trust settings, the Windows certificate store, the system's certificate files on
  Linux. `sslrootcert = "system"` says so explicitly.
- The name checked is the profile's host, also through an SSH tunnel (never `127.0.0.1`). An
  IP address is checked against the certificate's IP addresses.
- The client certificate and key are PEM files (PKCS#8, RSA or EC keys). A key protected by a
  passphrase is refused, with how to make an unprotected copy.
- PostgreSQL's SCRAM login binds itself to the TLS connection (`SCRAM-SHA-256-PLUS`) where the
  server offers it, so a server in between that passes the login on is found out. One that
  answers itself can still ask for the password (cleartext or MD5) unless the certificate is
  checked: use `verify-full`, or `channel_binding=require` in the URL.
- Cancel requests (PostgreSQL) and the `KILL QUERY` connections (MySQL) are encrypted as the
  session is.

## Defaults

- A PostgreSQL profile without `sslmode` connects with `disable`, as it always has; the form
  writes `prefer` for a new profile.
- A MySQL profile without a mode uses `PREFERRED`, except one that names a server public key
  file, which uses `DISABLED` (see [MySQL](mysql.md#passwords-on-a-direct-connection)). In a
  MySQL profile, `disable` and `prefer` (what older versions of the form wrote) count as no
  mode: write `DISABLED` to turn TLS off.
- A mode the driver does not know is an error of the profile.

## What a connection says

Each connection says what it got: `Connected to prod · TLS 1.3, server verified` (or
"certificate not checked", "CA checked"), a shield and the TLS version in the editor's
connection line, and the same in the form's test. A connection to another machine without TLS,
not through a tunnel, says it is not encrypted.

Failures say which: the server has no TLS; the certificate is not trusted, is for another name,
has expired or is not valid yet, or is not meant for a server; or the server refused the client
certificate. A file that cannot be read is named.

## Tested, and not

Tested: every mode against PostgreSQL 17 and MySQL 8.4 with certificates made for the tests
(trusted, another CA's, another name; client certificates accepted and refused on PostgreSQL),
through a dialer as a tunnel uses one, and PostgreSQL `verify-full` through a real SSH bastion;
in-process tests for expired and not-yet-valid certificates, other purposes, a self-signed
certificate as its own CA file, and TLS 1.2.

Not tested end to end: a server the system's certificates trust (only their refusal of an
unknown CA is tested), `sslrootcert = "system"`, real company servers, PgBouncer with TLS
between datarig and the pooler, and Windows.

## Limits

- Not yet: passphrase-protected client keys, certificate revocation lists, and
  `sslnegotiation=direct` (refused).
- Only TLS 1.2 and 1.3 are offered: no TLS 1.0 or 1.1.
- On macOS, `verify-full` against the system's certificates follows Apple's rules, which
  refuse server certificates valid for more than 825 days from a CA you added yourself: name
  that CA as the profile's CA file instead.
