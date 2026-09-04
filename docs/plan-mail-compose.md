# Plan — Mail: reply and compose (phase 7)

> Status: **built** (steps 1-5 of §10). `e r` answers the message under the
> cursor and `e n` writes a new one; both go out over SMTP, land a copy in
> Sent and set `\Answered` on what they answered. What is left is the live
> smoke run in §12 and the two open points marked below. This is the detailed
> plan for phase 7 of
> [`plan-mail-adapter.md`](plan-mail-adapter.md) §9 — the first phase in which
> the mail adapter _writes_. Phases 0-3 (folders, messages, bodies,
> attachments, the Thunderbird importer) are built; 4-6 (envelope cache, flag
> writes, IDLE) are not, and this plan pulls exactly one piece of phase 5
> forward: setting `\Answered` on the message that was answered.

## 1. What this adds

Two actions on a message level, both `InputSpec::Editor`:

| Action    | Key   | Buffer                                                  | Sends as                       |
| --------- | ----- | ------------------------------------------------------- | ------------------------------ |
| `reply`   | `e r` | headers, empty body, the original quoted below a marker | plain or HTML — see §3         |
| `compose` | `e n` | headers, empty body, no quote                           | HTML (config `compose_format`) |

Everything the user writes is Markdown. Everything the adapter sends is a
proper MIME message built in-process — no script, no pandoc, no shell. Sending
is an adapter capability, and a send path that depends on a helper script on
disk would be a side door around the one interface the CLI and the TUI share.

## 2. The buffer

```
From: Me <me@example.invalid>
To: Anna Beispiel <anna@example.invalid>
Cc:
Subject: Re: Angebot Q4

<the reply, written as Markdown>

>>> quoted from the original message — sent verbatim, edits below are discarded
> On 2026-09-03 14:22, Anna Beispiel <anna@example.invalid> wrote:
>
> Hallo,
> ...
```

- **Header block**: `Key: value` lines until the first blank line. `From`,
  `To`, `Cc`, `Bcc`, `Subject` — nothing else, and an unknown key is an error
  rather than a silently dropped instruction. `From` is shown even with one
  account: it says who is about to send, and it is how a multi-account instance
  is told _which_ account sends (the address is matched against the configured
  accounts).
- **Reply-all** needs no second action: the addresses are right there. A
  `reply_all` that prefills them is a later convenience, not a new mechanism.
- **The marker line** is a single fixed line (`QUOTE_MARKER`). Everything from
  it to the end of the buffer is the quoted region.
- **The guard** (the decision from the design discussion): on save the adapter
  compares the quoted region with the one it wrote — it gets both, because
  `ActionInput::Edited` carries the edited text _and_ the original template.
  - identical → the original's **HTML** is quoted (§4), not this text;
  - **removed entirely** (marker line gone) → a deliberate "reply without
    quote": nothing is quoted, and that is not an error;
  - **modified** → refused, with a message that says what happened and what to
    do. Silently discarding an inline reply somebody just wrote is the one
    outcome this whole design exists to prevent.
- **The draft survives.** `EditorPrep.file_path` puts the buffer in a real file
  (`<data>/not_yet_done/mail/drafts/<account>/<name>.md`) instead of a temp
  file, and `prepare` re-opens an existing draft instead of overwriting it. A
  long reply whose SMTP handshake fails is then one `e r` away, not gone. The
  file is deleted once the message is actually out.

  Both frontends honour it. The CLI opens `$EDITOR` on that same file, and
  writes text handed to it with `-m`/`--file`/stdin there as well: the buffer
  is the file, whoever filled it, so a refused send leaves something to come
  back to no matter which frontend wrote it.

- **An account that cannot send says so before the editor opens.** The missing
  `smtp:` block and the missing `address:` are both settled in `prepare`, not
  in `deliver`. There is nothing about either that only an attempt could
  reveal, and the alternative is a page of text written into a mailbox that
  was never able to send it.

## 3. Plain or HTML

Decided: **HTML as soon as the original has a `text/html` part**, even when a
`text/plain` alternative sits next to it. That is the common shape of business
mail, so in practice most replies are HTML — the plain path is for lists,
cron mails and people who mean it.

An HTML reply always ships a `text/plain` alternative as well, the way every
mail client does. It costs nothing here: the Markdown source _is_ the plain
part, plus the original quoted with `> ` prefixes. A recipient reading text
sees text, not an empty mail.

A new message (`e n`) has no original to take the decision from, so it follows
`compose_format:` — `html` by default, which is what Thunderbird and Gmail do.

## 4. The quote

Two conversions, in opposite directions, and they are not symmetric:

1. **For reading while writing** — the original goes into the buffer as
   Markdown (HTML mail, via `htmd`) or as its plain text (plain mail), every
   line prefixed with `> `, under an attribution line. Fidelity does not
   matter here: this text is _never_ sent. It only has to be readable.
2. **For sending** — the original's own HTML is wrapped in
   `<blockquote type="cite">` and appended below the reply, nested inside
   whatever quotes the original already carried. That is the convention every
   client follows, and it is why a mail thread reads as a thread.

Details that decide whether the result looks right in the recipient's client:

- **Quote styling** follows the de-facto convention (Gmail, Thunderbird):
  `border-left: 1px solid #ccc; margin: 0 0 0 .8ex; padding-left: 1ex` as an
  _inline_ style. Mail clients strip `<style>` blocks; inline styles survive.
  The user's own Markdown blockquotes (`> …`) get the same treatment, so a
  quote one wrote and a quote one inherited look alike.
- **Only the original's `<body>` content is quoted**, not its `<html>`/`<head>`.
  A whole document nested in ours is invalid, and the original's `<style>`
  block would restyle _our_ half of the mail — so `<script>` and `<style>` are
  dropped, everything else is kept as the sender wrote it.
- **Inline images** (`cid:` references) are re-attached with their original
  Content-IDs, which is what Thunderbird does — otherwise every logo and
  screenshot in the quote becomes a broken-image box. Configurable via
  `quote_images: attach | placeholder` for the account that would rather not
  carry a megabyte of somebody else's signature back and forth.

## 5. The message that goes out

```mermaid
flowchart TD
    B[edited buffer] --> H[headers]
    B --> M[Markdown body]
    B --> Q{quoted region}
    M --> P[text/plain part]
    M --> R[comrak → HTML]
    Q -->|unchanged| O[original body HTML]
    Q -->|removed| N[no quote]
    Q -->|modified| X[refuse, keep the draft]
    O --> P
    O --> R
    R --> A[multipart/alternative]
    P --> A
    O --> I[inline parts → multipart/related]
    I --> A
    H --> A
    A --> S[SMTP]
    S --> C[APPEND to Sent]
    C --> F[\Answered on the original]
```

Threading headers are not optional decoration — without them the reply hangs
outside the thread in the recipient's client:

- `In-Reply-To`: the original's `Message-ID`.
- `References`: the original's `References` chain plus its `Message-ID`.
- `Subject`: `Re: ` prefixed unless it already starts with `Re:` (case
  insensitive). `AW:` and friends are _not_ stripped — the recipient's client
  wrote them, and rewriting somebody's subject is not our call.
- `Message-ID`: minted here, in the `From` address's domain, rather than left
  to the library's hostname guess.
- `To`: the original's `Reply-To` if it has one, else its `From`. A message in
  the account's own Sent folder is answered to its `To` instead — replying to
  oneself is not what that gesture means.

## 6. Sending, and what happens after

- **SMTP** via `lettre`, one transport per account, built on demand.
- **Credentials**: the `smtp:` block may carry its own `auth:`; without one it
  reuses the account's IMAP credentials, which is the normal case (same
  provider, same login) and keeps the common config to three lines.
- **The Sent copy is our job.** SMTP hands the message to a server and forgets
  it; nothing appears in Sent unless we `APPEND` it there with `\Seen`. The
  folder is found via `LIST` `\Sent` (SPECIAL-USE), overridable with
  `sent_folder:` for servers that advertise nothing.
- **`\Answered`** is set on the original via `UID STORE` — the ↩ glyph the
  message list already has a slot for.
- **Failure semantics**: the send is the only step that can fail the action. A
  Sent copy that could not be appended, or a flag that would not stick, is
  reported as a warning on an otherwise successful send. Telling the user
  "sending failed" after the mail has left is worse than telling them the copy
  is missing — the first invites a second send.

## 7. Config surface

New, all optional except `smtp.host`:

| Key                   | Where              | Default         | Why                                                                                                                              |
| --------------------- | ------------------ | --------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `smtp.host` / `.port` | account            | — / by security | Sending is a different server from reading; guessing a port from a security mode is what phase 0 already refused to do for IMAP. |
| `smtp.security`       | account            | `tls`           | Same three modes as IMAP (`tls`/`starttls`/`none`), explicit for the same reason.                                                |
| `smtp.auth`           | account            | IMAP's          | A provider whose submission login differs. Empty in the common case.                                                             |
| `smtp.from_name`      | account            | account name    | The display name in `From`.                                                                                                      |
| `sent_folder`         | account            | SPECIAL-USE     | Servers that advertise no `\Sent`, and mailboxes whose Sent folder is not where the server thinks.                               |
| `compose_format`      | instance + account | `html`          | What a _new_ mail is sent as. A reply takes it from the original instead.                                                        |
| `quote_images`        | instance + account | `attach`        | Whether the quoted original's inline images travel back with the reply.                                                          |
| `reply_attribution`   | instance           | see §4          | The `On <date>, <who> wrote:` line — a template, because it is the one string in an outgoing mail that is pure convention.       |

## 8. Libraries

Liveness checked 2026-09-04.

| Purpose           | Pick                                                   | Stars  | Last push  | Why                                                                                                                           |
| ----------------- | ------------------------------------------------------ | ------ | ---------- | ----------------------------------------------------------------------------------------------------------------------------- |
| SMTP + MIME build | **lettre 0.11** (<https://github.com/lettre/lettre>)   | 2 257★ | 2026-08-03 | Already the choice recorded in `plan-mail-adapter.md` §3. Builds `alternative`/`related` itself, so no separate MIME builder. |
| Markdown → HTML   | **comrak 0.54** (<https://github.com/kivikakk/comrak>) | 1 692★ | 2026-08-30 | CommonMark **plus GFM** — tables and autolinks are what people actually write in mail.                                        |
| HTML → Markdown   | **htmd 0.5.5** (<https://github.com/letmutex/htmd>)    | 454★   | 2026-09-04 | For the quote _in the buffer_ only, so readability is the whole requirement.                                                  |

Rejected: **pulldown-cmark 0.13** (<https://github.com/pulldown-cmark/pulldown-cmark>, 2 706★, 2026-08-17) — leaner and faster, but plain CommonMark; the GFM table extension would have to be hand-built. **html2md** (<https://gitlab.com/Kanedias/html2md>, 21★, 2026-08-20) and **fast_html2md** (<https://github.com/spider-rs/html2md>, 79★, 2026-04-30) — much smaller user base than htmd, the latter quiet since April. **mail-builder** (Stalwart, 82★, 2026-08-18) — unnecessary next to lettre, and `mail-parser` from the same house is already the read side.

## 9. Module layout

- `compose/buffer.rs` — render the buffer, parse it back, the quote guard. Pure
  string work, no network, no MIME: the part that is easiest to get subtly
  wrong is the part that is cheapest to test.
- `compose/quote.rs` — original → Markdown quote, attribution line.
- `compose/render.rs` — Markdown → HTML, blockquote styling, body extraction
  from the original, threading headers, the finished `lettre::Message`.
- `compose/send.rs` — the SMTP transport, the Sent `APPEND`, `\Answered`.
- `imap/ops.rs` + `imap/conn.rs` — two commands more: `APPEND` and `UID STORE`.
- `adapter/message.rs` — the two actions, `prepare`, `execute`.
- `config.rs` — the block from §7.

## 10. Phase cut

| Step | Content                                                                        | Done when                                                                 |
| ---- | ------------------------------------------------------------------------------ | ------------------------------------------------------------------------- |
| 1    | Config (`smtp:`, `sent_folder`, `compose_format`, `quote_images`, attribution) | The example YAML parses; a bad block is rejected with a readable message. |
| 2    | Buffer: render, parse, guard                                                   | Unit tests cover header parsing, the three guard outcomes, resume.        |
| 3    | Quote + render: Markdown ↔ HTML, `<blockquote>`, threading headers             | An invented `.eml` fixture produces a MIME message with the right parts.  |
| 4    | SMTP send, Sent `APPEND`, `\Answered`                                          | A reply lands in the recipient's mailbox and a copy in Sent.              |
| 5    | Actions wired (`e r`, `e n`), view YAML, docs, smoke block                     | Both keys work in the TUI.                                                |

All five are built. Two things came out differently from the plan above, both
in §11.

## 11. Risks and open points

- **`From` is checked, not obeyed** — a deviation from §2, decided while
  wiring the actions. The plan had the `From` line select which account sends.
  It does not: the account whose subtab the mail was written in sends, and a
  `From` naming a different account is refused by name ("open the other
  account's subtab to send as it"). The reason is that an outbox belongs to
  one account and an account's connection is only opened when somebody opens
  its subtab — sending as another mailbox would need that mailbox's connection
  too, for the Sent copy and the `\Answered` flag, and would quietly log a
  second account in on a keystroke that says nothing about logging in. Picking
  the account by address stays possible; it needs a registry over all outboxes
  and is worth doing only if the "wrong subtab" mistake actually happens.
- **The quote guard is stateless, and had to be.** `ActionInput::Edited`
  carries the template the editor was opened with — but a RESUMED draft is its
  own template, because the frontend writes `template` to `file_path`. So the
  guard would have compared the second attempt against itself and waved
  through exactly the edit it refused the first time. It therefore re-renders
  the expected quote from the original (which is fetched anyway to build the
  outgoing HTML) and compares against that.
- **The guard refuses rather than asks.** An editor submit has no confirmation
  channel — `ActionDispatch::Confirm` belongs to the shortcut path, not to
  `execute` — so a modified quote is a refusal with instructions, and the
  draft is kept. If that turns out to be annoying in daily use, the honest fix
  is a confirmation on the editor-commit path, not a quiet send.
- **A wrong `From` sends nothing.** An address in the header block that no
  account carries is refused; guessing an account for an address the user
  invented would send mail from the wrong mailbox.
- **`quote_images: attach` can make a reply large.** Three rounds of a thread
  with a signature logo carry the logo three times. That is what every client
  does; `placeholder` is the way out.
- **No attachments on a new mail yet.** `e n` composes text; attaching files is
  a file-picker flow of its own, and forwarding (which needs the original's
  parts) is a separate action.
- **Sending is irreversible.** There is no undo and no send-later queue. The
  guard, the `From` check and the explicit header block are the whole safety
  net, and that is on purpose: a confirmation dialog on every mail is a
  keystroke people learn to skip.

## 12. Tests and smoke

- **Unit**: header parsing (folded values, unknown key, missing blank line),
  the three guard outcomes, `Re:` prefixing, `Reply-To` precedence,
  `References` chaining, body extraction from a document with `<head>` and a
  `<style>` block, `cid:` re-attachment, and a full build against invented
  `.eml` fixtures — never a real mail (`handling-real-data`).
- **Headless**: the built message is asserted as MIME text; no server needed.
- **Smoke**: one block in [`smoke-tests.md`](smoke-tests.md) covering both
  keys, the guard, the plain/HTML decision, the Sent copy, the ↩ flag and a
  failing SMTP host.
