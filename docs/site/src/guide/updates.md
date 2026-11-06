# Updates

datarig tells you when a newer release is out, and shows what changed after you update. Both
can be turned off.

## The update check

Once a day at most, when it starts, datarig asks GitHub for its latest release. It asks after
the screen is drawn, so starting is never slower, and quitting never waits for it.

**What is sent.** One HTTPS request:
`GET https://api.github.com/repos/DokaDev/datarig/releases/latest`, with the headers
`User-Agent: datarig`, `Accept: application/vnd.github+json`, `X-GitHub-Api-Version` and,
after the first answer, `If-None-Match` with the tag GitHub gave last time. Nothing else: no
identifier, no version, no operating system, no configuration, nothing about your connections
or databases. GitHub sees your IP address, as for any request (see GitHub's privacy statement).
The server's certificate is checked against the certificates your system trusts.

**Proxies.** The request goes through the HTTP proxy in `HTTPS_PROXY` (or `https_proxy`), else
`ALL_PROXY` (or `all_proxy`), with the user name and password the address gives, except for
hosts `NO_PROXY` lists. A proxy of another kind (SOCKS, HTTPS) means no check at all, never a
direct connection. The SSH tunnels and settings of your connections are never used.

**When it fails.** Offline, behind a firewall, without an answer within a few seconds, or when
GitHub limits requests from your address: nothing is shown, and datarig tries again the next
day. There is one attempt a day, whether it worked or not, and no retries.

datarig keeps what it learnt in `update.toml` in its state directory (see
[where datarig keeps its files](../faq.md#where-does-datarig-keep-its-files)): when it last
asked and how that ended, GitHub's tag for the answer, the latest version with its date and the
opening paragraph of its notes (as one line, at most 300 characters), the newest version that
ran here, a version you skipped, and when you last closed the notice.

## The notice

When a newer release is known, the next start shows a notice, unless you skipped that version,
closed its notice in the last week, or turned the checks off. It shows the new version and
yours, the opening paragraph of its notes (when they open with one), how to update your install,
and where What's new is.

| Installed with | The notice says |
|---|---|
| Homebrew | `brew upgrade dokadev/tap/datarig` |
| From the source: a binary in Cargo's `bin` folder (`$CARGO_HOME/bin`, else `~/.cargo/bin`) or in a `target/release` or `target/debug` folder | `git pull`, then `cargo install --locked --path crates/tui` |
| A release archive, or anything else | Where to download it (the latest release's page) |

| Key | What it does |
|---|---|
| `Enter` | Presses the button with the focus (Close at first) |
| `Esc`, `q` | Close: the notice comes back after a week |
| `s` | Skip this version: not told about it again (a newer one is) |
| `o` | Turn off the checks (saved as `[updates] check = "off"`) |
| `Tab`, the arrows, `h`, `l` | Move the focus between the buttons |

The notice never covers the welcome panel's first steps, and comes after What's new when both
are due. A newer release found while datarig runs is said on the status line only; the notice
waits for the next start.

`:update` shows the notice when a newer release is known (even one you skipped or closed), and
otherwise says what is known: that the checks are off and why, that datarig is up to date and
when it last asked, or that it could not ask and why (it never says "up to date" after a check
that failed).

## Turning it off

Any of these turns the check off:

* `[updates] check = "off"` in the config file, `:set updates.check=off`, or the settings
  screen (`Space ,`, Updates);
* `o` in the notice;
* the environment variable `DATARIG_NO_UPDATE_CHECK` with any value but an empty one (it wins
  over the config): for a shell profile, a shared machine or scripts.

datarig does not check either when `CI` is set to a value that is not empty (a CI service), in a
development build (`cargo build` without `--release`), without a state directory, or when
`update.toml` cannot be read (it is then left as it is). A second datarig open at the same time does not ask: it shows the
notice from what the first one learnt, but does not keep your answer to it (closing it or
skipping the version is done in the first one).

## What's new

On the first start after an update, datarig shows the notes of every version since the one you
ran before. The notes come with the program: nothing is fetched, and they are the
[What's new](../whats-new.md) page of this site. `:whatsnew` shows them any time, and
`[updates] whats_new = "off"` stops them from showing by themselves. See
[Getting started](../getting-started.md#whats-new) for its keys.
