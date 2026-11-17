# paperless-cli

`paperless-cli` is now a Rust application with three output modes for
[Paperless-ngx](https://github.com/paperless-ngx/paperless-ngx):

- `tui`: interactive Ratatui dashboard and inspector
- `markdown`: LLM-friendly summaries for humans and agents
- `json`: structured machine-readable output

The Rust binary is now the primary runtime and CI target.

## Install

```bash
cargo build --release
./target/release/paperless --help
```

## Quick start

Configure access with a token:

```bash
paperless login --url https://paperless.example.com --token YOUR_TOKEN
```

Or exchange username and password for a token:

```bash
paperless login --url https://paperless.example.com --username admin --password secret
```

If you omit the URL or auth fields, `paperless login` will prompt for them interactively.

Launch the TUI:

```bash
paperless
```

Launch the built-in demo mode for fast UI feedback without a live Paperless
server or local config:

```bash
paperless --demo
paperless --demo document list
paperless --demo --output json status
```

Request markdown or JSON output:

```bash
paperless --output markdown document list --query "invoice 2024"
paperless --output json search query "invoice acme"
paperless status
paperless dashboard
```

Set the HTTP timeout in seconds with `--timeout` or a `timeout` key in the
config file. The default is 30; `0` disables it. It bounds connecting, the wait
for a response, and each pause while a response body arrives, so a long
download that keeps receiving data is not cut off. Downloads stream to disk.
An upload fails once neither the file nor the response has moved for the
timeout, so a long upload is not cut off while data flows. After the last byte
is sent, the server must answer within the timeout. The login token request
uses the same timeout.

```bash
paperless --timeout 120 document upload scan.pdf
```

## TUI overview

The default TUI is a two-pane document browser:

- `Tab` switches focus between the Documents list and the Inspector
- `j/k` or arrows move within the focused pane
- `PgUp/PgDn` page through the focused pane
- `g/G` jump to the top or bottom of the focused pane
- `r` reloads the document list

For fast iteration on layout and keyboard behavior, use `paperless --demo`.

![paperless-cli TUI demo](site/assets/demo-tui.png)

## Command surface

| Area | Commands |
| --- | --- |
| top level | `login`, `status`, `dashboard`, `api`, default TUI |
| `config` | `set-url`, `set-token` |
| `pdf` | `read`, `info` |
| `project` | `login`, `info`, `ping` |
| `document` | `list`, `get`, `content`, `upload`, `download`, `preview`, `thumb`, `edit`, `update`, `delete`, `search`, `bulk` |
| `search` | `query`, `autocomplete` |
| `task` | `list`, `get` |
| `tag` | `list`, `get`, `create`, `edit`, `delete` |
| `correspondent` | `list`, `get`, `create`, `delete` |
| `doctype` | `list`, `get`, `create`, `delete` |
| `export` | `bulk` |
| `trash` | `list`, `restore`, `empty` |

### Updating documents

`document update` and `document edit` set fields with `--title`,
`--correspondent-id`, `--type-id`, `--storage-path-id` and `--created`.
`update` replaces tags with `--tag-id`; `edit` adds and removes tags with
`--add-tag` and `--remove-tag`. Clear a field with `--no-correspondent`,
`--no-type`, `--no-storage-path` or `--no-tags`. Each clear flag conflicts with
its value flag.

```sh
paperless document update 42 --storage-path-id 3 --tag-id 1 --tag-id 2
paperless document update 42 --no-correspondent --no-tags
```

### Bulk edits

`document bulk` changes many documents at once. It sends one request per
operation, in this order: tags (`add_tag`, `remove_tag`, or one `modify_tags`
when several tags change), `set_correspondent`, `set_document_type` and
`set_permissions` through `documents/bulk_edit/`, then `documents/reprocess/`.
It stops at the first failed request and names the operations already applied.
`-` reads IDs from stdin. Duplicate IDs are dropped. `--dry-run` prints the
requests and sends nothing.

```sh
paperless document bulk 1 2 3 --add-tag 5 --remove-tag 7
paperless document bulk 1 2 --correspondent none --type 4
paperless document list --json | jq '.data.results[].id' | paperless document bulk - --reprocess
paperless document bulk 1 --set-permissions --owner 2 --view-groups 3,4 --dry-run
```

`--set-permissions` sets the owner and needs `--owner`. `--owner none` leaves
the documents without an owner, which makes them visible to all users. Each of
`--view-users`, `--view-groups`, `--change-users` and `--change-groups` replaces
that list; `none` empties it. A list not given stays as it is. Paperless keeps
no history of permission changes, so check with `--dry-run` first.

### Trash

`trash list` shows the ID, deletion time and title of each document in the
trash, across all pages. `trash restore` and `trash empty` take document IDs;
`-` reads them from stdin and duplicates are dropped. `trash empty` deletes the
documents for good, so it asks first. Pass `--yes` to skip the question; without
it, a non-interactive run refuses, and so does a run that reads IDs from stdin.
Neither command runs without at least one ID, because Paperless empties the
whole trash when the request names no documents.

```sh
paperless trash list
paperless trash restore 12 13
paperless trash empty 12 --yes
```

### Raw API calls

`paperless api` reaches endpoints the CLI does not wrap, with the stored URL and
token, so the token never has to enter the shell:

```sh
paperless api GET status/
paperless api GET documents/ --query title__icontains=invoice --query page_size=5
paperless api POST documents/bulk_edit/ --data '{"documents":[1],"method":"delete"}'
paperless api PATCH tags/3/ --data @tag.json
paperless api GET trash/ --all
```

`PATH` is relative to `/api/`; a missing trailing `/` is added. The response
JSON goes to stdout, a non-JSON body streams as is. A non-2xx status exits
non-zero with the response body on stderr. Only GET follows redirects, here and
in every other command, so a redirected write fails instead of turning into a
GET. `--all` follows `next` links of a paginated GET and prints the merged
`results` as one array. A `next` link must stay on the configured server.

### Network behavior

`paperless status` is a compact health and authentication probe. It sends only
`GET /api/status/` and does not fetch statistics, tasks, documents, or history.
The default terminal view uses ASCII formatting to summarize component health,
versions, migrations, and storage. `--output json` preserves the complete API
response for scripts.

`paperless dashboard` is the explicit full operational overview. It sends:

- `GET /api/status/`
- `GET /api/statistics/`
- `GET /api/tasks/`

Help is always local and network-free, including `paperless --help`,
`paperless help <command>`, and `paperless <command> --help`.

## Compatibility

The CLI now includes the main compatibility commands and inputs people expect
from [`julianfbeck/paperless-cli`](https://github.com/julianfbeck/paperless-cli):

- `document content <id>` for extracted text only
- `document edit <id> --add-tag ... --remove-tag ...` with exact tag names or IDs
- `tag edit <id> --name ...`
- `pdf read <file>` and `pdf info <file>`
- `config set-url ...` and `config set-token ...`
- `PAPERLESS_URL` and `PAPERLESS_TOKEN` support
- global `--json`, `-q/--quiet`, `--no-color`, and `-u/--url` compatibility flags

See [docs/cli-parity.md](docs/cli-parity.md) for the current compatibility notes.

## Security posture

- Config and session state are stored as TOML under the user config/state
  directories with restricted permissions on Unix.
- Downloaded filenames are sanitized before writing to disk.
- A background security reviewer polls shared runtime state and feeds findings
  back to the main app. Its default model profile is `gpt-5.4`.
- The app avoids shelling out for Paperless operations; all API calls go
  through the Rust client.

## Docs

- [Migration notes](docs/rust-migration.md)
- [Architecture overview](docs/architecture.md)
- [Testing strategy](docs/testing-strategy.md)
- [CLI parity notes](docs/cli-parity.md)
- [Release process](docs/release-process.md)
- [Static docs site](site/index.html)

## Skills

This repo now ships installable repo-local skills:

- [SKILL.md](SKILL.md)
- [skills/paperless-documents/SKILL.md](skills/paperless-documents/SKILL.md)
- [skills/paperless-pdf/SKILL.md](skills/paperless-pdf/SKILL.md)
- [skills/paperless-admin/SKILL.md](skills/paperless-admin/SKILL.md)

## Development

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```
