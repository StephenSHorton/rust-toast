# The rust-toast program: protocol 1

`rust-toast` shows the library's cards for programs not written in Rust.
An app spawns `rust-toast serve`, keeps it running, writes commands to its
stdin and reads events from its stdout. Each is one JSON object on one
line (newline-delimited JSON, UTF-8). stderr carries logs only.

Build it with the `cli` feature (the library alone pulls in nothing new):

```sh
cargo build --release --features cli      # target/release/rust-toast(.exe)
```

Tagged releases attach `rust-toast-windows-x64.exe` and
`rust-toast-macos-universal` (arm64 + x64), with `SHA256SUMS`. They are not
code-signed; an app that ships one signs it with its own identity.

[`examples/node-client.mjs`](examples/node-client.mjs) is a working client
in about 70 lines of Node with no dependencies.

## Life cycle

- On start it writes `ready`. Wait for it before relying on cards; commands
  sent earlier are read in order anyway.
- It exits when its stdin closes, so it can never outlive the app that
  spawned it (also when that app is killed), and on `{"cmd":"quit"}`. Its
  cards go with it. The exit status is 0.
- If stdout's reader goes away, it exits.
- On macOS it runs the AppKit loop on its main thread as an Accessory app:
  no Dock icon, no menu bar, never the active app; the cards never take
  focus. On Windows the cards run on the library's own UI thread.
- On any other system `ready` says `"supported": false` and every command
  is accepted and does nothing (posts still answer `posted`).

## Commands

Every command has `cmd`. Fields not listed are ignored, so a newer client
can talk to an older program. Fields left out take their defaults (for
`configure` and `set_options`: keep their current value).

| `cmd` | Fields | What |
| --- | --- | --- |
| `configure` | `app_name`, `scheme` (`light`, `dark`, `system`), `corner_radius` (dip, 0 to 24), `glass` (0 to 1; 1 is solid), `stripe`, `capture_exclude`, `shown`, `max_cards`, `light`, `dark` | The look. Set it once at startup; cards up restyle. `light` and `dark` take `background`, `foreground`, `muted`, `border` and `accent` (`info`, `success`, `warn`, `error`), each `#rrggbb`. |
| `set_options` | `enabled`, `corner` (`top-right`, `bottom-right`, `top-left`, `bottom-left`), `sound`, `volume` (0 to 1), `confetti` | The person's settings. `enabled: false` takes every card down. |
| `post` | see below | Shows a card, or replaces the one up with the same `id`. |
| `close` | `id` | Takes the card down (reported as `dismiss` with `how: "closed"`). |
| `list` | | Reports the ids of the cards up (`cards`). |
| `quit` | | Exits. |

`post`:

| Field | Default | What |
| --- | --- | --- |
| `id` | made up (`auto-1`, ...) | Names the card in its events. A post with the id of a card that is up replaces it in place (and restarts its timer); the old one reports nothing. |
| `title` | | One line. A post needs a title or a body; with only a body, the body becomes the title. |
| `body` | | Up to two lines; longer text is ellipsized. |
| `kind` | `info` | `info`, `success`, `warn` (or `warning`), `error`: colour, icon and sound. |
| `sticky` | false | Stays until the person or a `close` takes it down. |
| `timeout_ms` | 3 s, 6 s for warn and error, 10 s with buttons | How long it stays. Ignored when sticky. Hovering the stack pauses it. |
| `thumbnail` | | Path of a PNG or JPEG shown under the body. |
| `sound` | true | false: this card is silent. |
| `confetti` | the options' | true or false: a confetti burst from this success card, whatever `set_options` said. |
| `clickable` | true | A click on the body reports `click`. false: a body click is a dismiss. |
| `actions` | | Up to two buttons, `{"id": "...", "label": "..."}`; the first is primary. Extra ones are dropped. An empty id is the button's index (`"0"`, `"1"`); an empty label shows the id. |

Any click on a card (a button, the body, the x) takes it down.

## Events

Every event has `event`.

| `event` | Fields | When |
| --- | --- | --- |
| `ready` | `version` (the program's), `protocol` (1), `platform` (`windows`, `macos`, `linux`, `other`), `supported` | At start. |
| `posted` | `id` | A post took (its id, given or made up). |
| `action` | `id`, `action` | A button was pressed: the card's id and the button's id. |
| `click` | `id` | The card's body was clicked. |
| `dismiss` | `id`, `how` | The card went without a button or a body click; see below. |
| `cards` | `ids` | Answer to `list`: the cards up, oldest first. |
| `error` | `message`, `id` (when about a card), `command` (when about a line) | Something was wrong. The program carries on. |

Each card ends with exactly one of `action`, `click` or `dismiss`, except
a card replaced by a post with its id, which goes on under the new post.

`how`:

- `user`: the person dismissed it: the x, a right or Control click, a
  sideways throw, or a body click with `clickable: false`. The library
  does not tell these apart.
- `timeout`: it expired.
- `closed`: a `close` command, or `set_options` with `enabled: false`.
- `pushed`: a post past `max_cards` (default 5) pushed it off the stack
  (the oldest card that isn't sticky goes first).

## Errors and limits

- A line that isn't a JSON object, has an unknown `cmd`, or has a field of
  the wrong type is an `error` event; nothing else happens. `command` echoes
  the start of the line (at most 200 characters, control characters
  removed).
- A bad value (`corner`, `volume`, a colour) makes the whole command an
  error; nothing of it is applied.
- A line longer than 256 KiB is an `error` and skipped.
- Text is cut: `id` and button ids to 128 characters, `title` to 256,
  `body` to 2048, labels to 64, `app_name` to 64.
- `max_cards` and `shown` are capped at 50; `timeout_ms` at a day.
- A thumbnail is read only as an image: a regular file (not a directory,
  device or pipe) of at most 16 MiB whose first bytes say PNG or JPEG, at
  most 8192 pixels a side. Anything else is an `error` with the card's
  `id`, and the card goes up without it. Errors never quote the file.
- A post while cards are off (`enabled: false`) is an `error`.

## Example

```text
> (from the program)  < (from the app)
> {"event":"ready","version":"0.3.0","protocol":1,"platform":"macos","supported":true}
< {"cmd":"configure","app_name":"Hearth","scheme":"system","corner_radius":10}
< {"cmd":"set_options","corner":"bottom-right","volume":0.4}
< {"cmd":"post","id":"update","title":"An update is ready","body":"Restart now, or it installs when you quit.","sticky":true,"actions":[{"id":"restart","label":"Restart"},{"id":"later","label":"Later"}]}
> {"event":"posted","id":"update"}
< {"cmd":"post","title":"Saved","kind":"success","timeout_ms":2000}
> {"event":"posted","id":"auto-1"}
> {"event":"dismiss","id":"auto-1","how":"timeout"}
> {"event":"action","id":"update","action":"restart"}
< {"cmd":"post","title":
> {"event":"error","message":"bad command: EOF while parsing a value at line 1 column 22","command":"{\"cmd\":\"post\",\"title\":"}
```

## One card from a script

```sh
rust-toast --title "Backup finished" --body "42 GB in 6 min" --kind success
rust-toast --title "Deploy?" --sticky --action yes:Deploy --action no:Cancel
```

It waits until the card is gone and prints its one closing event (`action`,
`click` or `dismiss`, as above), then exits 0; 1 when the card could not be
shown (with an `error` line), 2 for bad arguments. `rust-toast --help`
lists the flags: `--title`, `--body`, `--kind`, `--sticky`,
`--timeout SECONDS`, `--silent`, `--action ID:LABEL` (twice at most),
`--thumbnail`, `--corner`, `--app-name`.

## Versions

`protocol` goes up only when a command or event changes meaning. New
optional fields, commands and events don't bump it; clients ignore what
they don't know.
