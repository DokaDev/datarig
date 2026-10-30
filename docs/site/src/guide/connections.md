# Connections

A **connection profile** says how to reach one database server: driver, host, port, user,
database, how to get the password, and optionally TLS, an SSH tunnel and an environment. The
explorer's **Connections** section lists every profile, in nested folders, each with its own
color and icon.

```text
{{#include ../screens/profile-form.txt}}
```

## Make and edit profiles

- `Space c n` (or `n` in the explorer) opens a new profile form; `e` in the explorer edits the
  selected profile, `c` duplicates it, `d` deletes it, `m` moves it to a folder and `N` makes a
  folder.
- A connection URL can be pasted into the form, for example
  `postgres://me@db.internal:5432/app?sslmode=verify-full`. MySQL URLs start with `mysql://`
  (or `mariadb://`). A URL with several hosts is refused.
- **Test** (`t` in the explorer, `Space c t` for the active tab's connection) connects once
  and shows the server version, the latency, how the connection is encrypted and, through a
  tunnel, the tunnel's time.

## Connect and switch

- `Ctrl+O` (or `Space c c`) opens quick connect: a fuzzy list of the profiles. `Enter` goes to
  the profile's most recently used tab, or connects and opens a console when it has none.
  `Right` on a profile lists its server's databases (and on a database its schemas), `Left`
  goes back; `Enter` there opens a console in that database or schema.
- `:conn <profile>` does the same from the command line, and `datarig <profile>` at start.
- `o` on a profile in the explorer opens a new console on it.
- Every tab has its own database and schema: `:use db.schema` or `Space c d` picks them.
  `Space c s` moves the tab to another connection.
- `Space c x` disconnects the tab's connection and `Space c r` reconnects it.

Several connections can be open at once. Each has its own metadata session, schema tree and
completion catalog, and each tab belongs to one connection. A tab's number shows its
connection's color while connected, is muted while not, and turns into a spinner while a
statement runs. `◆` after a tab's name marks an open transaction of yours, `!` an aborted
transaction or a lost connection.

## Passwords

Passwords are never written to the config file. Each profile picks a source with
`password_source`:

| Source | Where the password is |
|---|---|
| `keychain` (the default) | The OS credential store: macOS Keychain, Linux Secret Service, Windows Credential Manager. |
| `file` | `secrets.toml` next to the config file, readable by you only (mode `0600`). |
| `command` | The standard output of `password_command`, for example a password manager's CLI. |
| `env` | The environment variable named by `password_env`, read at connect time. |
| `prompt` | Asked on every connect, never stored. |

- Every keychain call runs off the UI thread with a 10-second limit, so a locked or slow
  keychain cannot freeze the app. When the keychain does not work, datarig asks for the
  password when connecting and keeps it in memory only.
- A password command is split into words like a shell would, but runs **without a shell**:
  no pipes, variables or `~`. Write `sh -c '…'` when you need them. It gets 10 seconds, and
  one trailing newline is removed from its output.
- `[secrets] default_source` (or `:set secrets.default_source=prompt`) picks the source new
  profiles start with; `auto`, the default, is the keychain when it works.
- A plaintext `password` found in an older config file is moved to the profile's store at
  launch and removed from the file.
- `DATARIG_SECRET_STORE=memory` keeps every password in memory only, for this run; it replaces
  both the keychain and the secrets file.

### macOS: keychain access through `security`

datarig's release binaries are not signed. macOS ties the "Always Allow" you give a keychain
item to the program that asked, so after every upgrade it sees a new program and asks for your
login password again for each saved password.

To avoid that, datarig can reach its keychain items through Apple's own `/usr/bin/security`
tool, which is signed by Apple and does not change when datarig is upgraded:

```toml
[secrets]
macos_keychain_access = "security_tool"   # direct (the default) | security_tool
```

The same setting is on the settings screen (`Space ,`, Secrets group) and is
`:set secrets.macos_keychain_access=security_tool`. It applies at once; turning it on asks
first.

- **What to expect.** The first time each existing password is read or changed through
  `security`, macOS may ask once whether `security` may use it: enter your login password and
  choose **Always Allow**. After that, upgrades no longer ask.
- **The trade-off.** Any program running as your user can then read these passwords with
  `security` (for example `security find-generic-password -s datarig -g`) without being asked.
  With `direct`, macOS asks before another program reads them.
- **Limits.** In this mode a password containing a line break, or longer than about 3.5 KB,
  cannot be saved (datarig says so and saves nothing). Passwords saved earlier still read.
- **The items are the same** in both modes (generic passwords, service `datarig`, account
  `profile:<id>`, in your default keychain), so switching moves nothing. Passwords are handed
  to the tool on its standard input, never on its command line.
- **Switching back** to `direct` keeps every password working; items saved while
  `security_tool` was on trust only `security`, so macOS may ask once per item.
- The setting is accepted on every platform and has no effect outside macOS.

**The one-time offer.** In `direct` mode on macOS, when a keychain call most likely waited for
you to approve it after an upgrade, datarig asks once whether to switch to `security_tool`,
after the operation that used the keychain has finished. `y` switches, `n` keeps `direct` for
good, `Esc` or `Enter` keep `direct` and decide later.

- A call counts as such a prompt when it took at least one second (or hit the 10-second limit)
  **and** the binary is not the one that last read a saved password directly. A slow keychain
  with an unchanged binary does not count.
- If you answer the macOS dialog in under a second, the prompt is missed and the offer comes
  at the next one.
- It is never offered outside macOS, over SSH (`SSH_CONNECTION`, `SSH_CLIENT` or `SSH_TTY`
  set), in `security_tool` mode, with `DATARIG_SECRET_STORE=memory`, or once answered.
- The answer is kept in `keychain.toml` in the state directory, not in the config file.
  Deleting that file makes datarig forget it.

A signed release would be the proper fix; until releases are signed, this setting is the
workaround.

## The config file

The config file is `--config <path>`, else `$XDG_CONFIG_HOME/datarig/config.toml`, else
`~/.config/datarig/config.toml`, on every OS. The app creates and edits it (comments and key
order are kept), and you can write it by hand. Every key is listed on the
[configuration reference](../reference/configuration.md) page. A profile looks like this:

```toml
[[connections]]
name = "prod-replica"
driver = "postgres"            # postgres | mysql
host = "db.internal"
port = 5432
user = "readonly"
database = "app"
color = "red"
folder = "work/prod"           # nested folders are written with /
policy = "prod"                # its environment (see Environments and safety)
password_source = "command"
password_command = "my-password-tool get prod-db"   # your password manager's command line
tunnel = "office"              # a tunnel preset (see SSH tunnels)
sslmode = "verify-full"        # see TLS
```

Related pages: [SSH tunnels](ssh-tunnels.md), [TLS](tls.md), [Poolers](poolers.md),
[Environments and safety](environments.md), [Startup SQL](startup-sql.md).
