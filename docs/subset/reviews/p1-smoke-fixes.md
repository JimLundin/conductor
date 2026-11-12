# Review: the p1-smoke-fixes round

This is the reviewer's pass over `3a61161` (`rv-p1-smoke-fixes`) against `main` at `a826e69`. The round fixes smoke run 1's findings 3 to 6 (`docs/p1/smoke.md`) and the open wire items in `docs/p1/design.md` §12, under `conductor/3` and subset version 7. I read the friction log (`.audit/p1-fixes.friction.md`), `claude/PROTOCOL.md`, `codex/PROTOCOL.md` and the Claude recordings under `fixtures/`. I did not run the browser e2e or the live hosts.

Every rewrite below was applied to a scratch copy of `3a61161`. `docs/subset/measurements/p1-smoke-fixes/` holds the patches, and its `measure.sh` reprints every count in this review.

The verdict is **accept after the required fixes**. There are five code fixes. Fix 1 is a real bug: "Thinking…" shows under an open approval. With the fixes, the screen files go from 5,803 to 5,720 tokens. One proposal, 0042, reaches the score bar.

## Compliance

`subset check` reports nothing on the branch. I did not rerun the whole of `tools/check.sh`. For the fixes, I ran `cargo clippy -p app -p client -p proto --all-targets -D warnings`, `cargo test -p client` and `cargo test -p daemon --lib mapping` on the scratch copy, and all three passed with fixes 1 to 3 applied. Fix 4 and fix 5 are untested.

The qualities hold except where a fix names a breach. One more point is not a fix. `pub mod device`, `pub enum Touch` and `pub fn device_name` are used only inside `client`, so 0002 would make them `pub(crate)`. `pub` is there to keep clippy quiet. Only `platform/web.rs` calls `device_name`, so with `pub(crate)` the native `cargo clippy -p client --all-targets -D warnings` fails with five `never used` errors (measured). That is a workaround and belongs in the friction log as point 4. The owner can rule on it there.

## Required fixes

1. **"Thinking…" shows while an approval waits.** `SessionView::working` hides the indicator only when the last item is a streaming reply or a running tool. An open request on an earlier item doesn't hide it. Two failing inputs, each run after `UserMessage`:
   - `Started(toolu_1)`, `Started(toolu_2)`, `Finished(toolu_2)`, `RequestOpened(toolu_1)`. This is Claude running a read-only tool beside a `Write` that asks. `attention()` is `Requests(1)`, and `working()` is `Shown`.
   - `Started(toolu_1)`, `AssistantMessage`, `RequestOpened(toolu_1)`. Same result.

   Match on `attention()` as well, so that `(Running, Requests(_), _)` is `Hidden` (`working.patch`, which adds both cases as tests). `cargo test -p client` passes with the patch, including the branch's own `Working` table.
2. **The provider note is mapped twice, and its error is written twice.** `NewSession` maps `ProviderNote` into a local `Note` enum, then matches `Note` to markup. It also formats `"{} can't run on this host: {reason}"` in two places, once for the refusal and once for `Unavailable`. One `match (provider_error, provider_note)` with the arm `(Some(reason), _) | (None, ProviderNote::Unavailable(reason))` removes `Note`, the second match and the second format. `provider_error` then carries the bare reason (`provider-note.patch`). `sessions.rs` goes from 1,552 to 1,477 tokens, and its score goes from 1,499.0 to 1,433.0.
3. **`Change::Create` duplicates `Write`.** Only Codex's `Add` produces it, and `Write`'s own doc says "new or replaced". Failing input: Claude writing a new `a.txt` shows "Write a.txt", while Codex adding the same file shows "Create a.txt". That is two words for one act. Map `Add` to `Write` and drop the variant (`no-create.patch`, -10 tokens). The wire is still unreleased `conductor/3`, so this needs no new ALPN. `Vec<FileChange { path, change }>` is otherwise the right shape. One Codex patch mixes kinds across files, so the change has to be per file, and a Claude tool always names one file.
4. **A Codex permissions request shows raw JSON.** `approval_tool` now sets `detail: requested.to_string()`, where `requested` is a `serde_json::Value`. Failing input: `Permissions { reason: Some(..), requested: {"network":{"enabled":true}} }` puts `{"network":{"enabled":true}}` in the card's code line. No test covers the arm. Summarise `requested` in `mapping`, or leave `detail` empty now that the reason has its own line. Either way, add the case to the mapping table.
5. **The Codex note leaves out the network.** With `OnRequest` and `WorkspaceWrite`, Codex runs sandboxed commands and writes inside the folder and `/tmp` unasked (`codex/PROTOCOL.md`, Approvals, and smoke run 1 finding 2). It also asks before it uses the internet. OpenAI's sandboxing page describes this mode as "Always ask to edit external files and use the internet" (<https://learn.chatgpt.com/docs/sandboxing>). Write: "Codex runs commands and edits files in this folder and in /tmp without asking. It asks before writing anywhere else or using the network." One more limit is a guess I did not verify. Recent Codex keeps `.git` inside a writable root read-only, so a commit would ask. Check it with `s2` before the sentence claims it.

## The focus questions

**Client name.** The mapping is right for current browsers. Reduced user agents still carry `iPhone`, `Android`, `CrOS`, `Windows NT 10.0` (also Windows 11) and `Macintosh`. iPadOS Safari's Mac user agent with more than one touch point becomes "iPad". There are two mislabels, and neither is worth code. An iPhone that requests the desktop site becomes "iPad". Android Chrome in desktop mode becomes "Linux". The name is decided once, in `platform::client_name`, over a pure `device_name`. It can only be one of eight fixed words, so no part of the user agent reaches a card. One wording point is for the owner. The native app still says "desktop" in lower case, so a card can read "Approved by desktop" above "Approved by iPhone".

**"Thinking…".** It is right in every state except an open approval (fix 1). The indicator shows after an interrupt until `TurnCompleted`, and for a held interrupt. Both are honest, because the agent is still finishing. It is hidden while a tool runs, after the turn ends, and once the session closes.

**`RequestOpened.reason`.** The Codex mapping is right for command, file-change and permissions requests. For Claude, `None` is right for now. Every `can_use_tool` in `fixtures/` carries `tool_name`, `display_name`, `description`, `input`, `permission_suggestions` and `tool_use_id`, and `description` is the file name (`"a.txt"`), which repeats the path. The vendored `ToolPermissionRequest` also has `decision_reason` and `title`, but no recording has either. Pass `decision_reason` through once a recording shows one. A Bash tool's `input.description` is the model's summary of the command, the nearest thing Claude has to Codex's reason. No recording has a Bash approval, because the CLI auto-allows read-only commands.

**Keyboard.** On a desktop, `env(safe-area-inset-bottom)` is 0, so the rule changes nothing. On an iPad with a hardware keyboard, focus drops the home indicator's 20 px inset while no keyboard rises, so the composer sits on the indicator until it loses focus. That is cosmetic. Safari has no better CSS signal. `env(keyboard-inset-height)` (VirtualKeyboard API) and the `interactive-widget` viewport key are Chromium only. The only reliable signal is `visualViewport`'s height in script, which costs more than this misfire. Keep the rule, and confirm it on the phone as `smoke.md` plans. The e2e check proves only the selector's specificity.

**Screenshot churn.** The pair text changes from run to run, and on the phone its length can change where the band wraps. Playwright 1.65's `screenshot` takes a `style` option. In the screens scenario, write `shot.screenshot({ path: file, style: '.status .path { display: none; }' })`. Use `display: none`, not `visibility: hidden`, because a hidden span keeps its varying width. That is one line, and it changes no app code.

## Volume

`crates/app/src` went from 5,502 tokens on `main` to 5,803. Fixes 2 and 3 bring it to 5,720. The rest is the features themselves: the reason line, the per-file verbs, the indicator and the note. `ticket.rs` lost 50 tokens because `client_name` moved into `client`. I found nothing else avoidable in v7. The `files.first()` arm in `ToolLine` covers a Codex file-change request for an item the view never saw start. It is rare, but it is real.

## Friction points

| # | Point | Ruling |
|---|---|---|
| 1 | The UA test table needs `String` fields | No proposal. `&'static str` fields make it longer: rows become struct literals, and the test goes from 279 to 298 tokens. v7 has a shorter form today. An inline `Vec::from([(ua, touch, name), ..])` that is never bound comes to 201 tokens, and its score goes from 273.5 to 230.0. It passes `subset check` (`device-test-tuples.rs`). It isn't required, because 0001 speaks of "short-lived pairs", and a ten-row triple stretches that. The owner can say whether an unbound test table counts. |
| 2 | Two `match`es over `Device` | Proposal 0042. A `const` table of `&'static str` rows, scanned in order, takes `device.rs` without its tests from 284.5 to 234.5 (-17.6%) and from 357 to 242 tokens. |
| 3 | `Vec::from([..]).into_iter()` | No proposal. 0015 rules it on purpose, and it costs one call. `new_session.rs`'s test writes `[..].map(..)` inside `assert_eq!`, where the checker reads nothing (`tools/subset/README.md`, "Macro bodies aren't checked"). That is a second form, through a known hole. |

**`&'static str` across reviews.** There are five points in three reviews:
- `p1-u2.md`, point 1: const-table fields.
- `p1-u6.md`, points 2 and 7: `label()`'s return type, where the owner fixed the doc.
- This round, points 1 and 2.

They are two kinds. Const-table fields have three points, and return types have two. By raw count, the field kind meets "three friction points of one kind". The bar asks for points the construct removes, though, and it shortens only one of the three. `p1-u2` measured its keyed tables at +99% and +116%, and point 1 here grows by 19 tokens. So the count is not evidence. 0042 stands on its score alone, with a rule narrowed to ordered scans, so that keyed lookups stay a `match`. The return-type kind stays below the bar at two points and 2 tokens a function (`p1-u6.md`).

## Proposals

- **0042**: `&'static str` fields in a `const` table scanned in order. Score -17.6% (284.5 to 234.5), tokens -32% (357 to 242), tier tolerated, 1 friction point.
