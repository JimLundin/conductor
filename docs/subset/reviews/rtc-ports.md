# Review: WebRTC port range and upgrade backoff (rtc-ports)

The reviewer's pass over `git diff main rv-rtc-ports` (tip `7a319fa`) and the friction log `.audit/rtc-ports.friction.md`, under subset version 7.

**Verdict: accept after one required fix.** The port range, the refusal and the backoff do what smoke run 1, finding 1 asks for. The socket leak is real and the `JoinSet` fixes it. The two new e2e tests bind fixed ports, so they fail when anything else holds those ports (required fix 1). There is no proposal.

## What I ran

- `SUBSET=1 tools/check.sh` at the tip: exit 0. `subset check` reports nothing in the changed files.
- In a scratch worktree, I restored the old detached readers (`tokio::spawn(read_socket(..))`, no `shutdown`). `a_full_range_refuses_the_upgrade_and_iroh_keeps_serving` then fails with `left: (None, Some(Rejected(UnknownSession)), [])`, `right: (.., [31321])`. The port stays held after the session ends, and the test catches it.
- Same worktree, `JoinSet` kept but `self.readers.shutdown().await` removed: the test passes 5 of 5 runs.
- Same worktree, `signal_send` moved back above `sockets` in `Driver`: the test passes 5 of 5 runs.
- With a Python process holding UDP `0.0.0.0:31311`, I ran the two new e2e tests on the tip. `concurrent_webrtc_sessions_take_distinct_ports_in_the_range` fails with `left: ([31312], [])`, `right: ([31311], [31312])`.
- `subset score --by file|fn` on the six changed files, before (`main`) and after.
- Microsoft's Hyper-V firewall page, the WSL networking page, the `.wslconfig` reference and the `New-` and `Remove-NetFirewallHyperVRule` cmdlet references, through WebFetch.

## Port allocation

**Two offers at once.** No race. `bind_sockets` is synchronous, and each candidate port is claimed by the `bind` call itself, with no await between choosing a port and binding it. The OS decides who wins, and the loser moves to the next port. `concurrent_webrtc_sessions_take_distinct_ports_in_the_range` shows two sessions getting 31311 and 31312.

**Another process in the range.** Handled. Linux refuses `192.168.0.27:p` while another socket holds `0.0.0.0:p` or `192.168.0.27:p` without `SO_REUSEADDR` (measured with Python, `Errno 98`), and the host doesn't set it. The host skips that port, as the run with `0.0.0.0:31311` held shows: the first session took 31312, and the second was refused. The host behaved correctly. Only the test's literal ports broke (required fix 1).

**The leak and its fix.** Before this change each reader was a detached task that ended only when a datagram arrived after the driver had gone. On a quiet socket it never did, so the port stayed bound for the life of the process (measured above). Now the readers live in the driver's `JoinSet`.

- *Panic in the driver.* Unwinding drops the `JoinSet`, which aborts the readers, so the ports come free shortly after. The ordering against the signaling stream is lost on this path, but the client waits at least 15 s before it offers again.
- *Early `?` in `accept`.* Every `?` comes before the readers are spawned. The sockets and the `Stun` probes are locals there, so returning drops them. After the spawn there is no `?` before the driver owns everything.
- *Client vanishes without closing.* The driver ends through `SignalingClosed` when iroh's connection to that client times out. Before ICE connects, `NotConnected` ends it after `CONNECT_TIMEOUT`. The ports are held until then, which is bounded.
- *The field order.* The two passing mutations show that the `JoinSet` does the work. `shutdown().await` makes "ports free before the stream closes" strict for the reader tasks, and it costs one line, so keep it. The reorder of `signal_send` only orders drops inside one synchronous `Drop`, which the client can't observe within a round trip. It is harmless, and its comment overstates what it guards. Optional: drop the move and the comment.

**IPv6.** No regression. `interface_ips` is unchanged from `main`, and it already kept IPv4 only (`i.ipv4`, `IpAddr::V4`). The old doc comment said "one socket per usable IPv4 interface" too.

## Refusal path

The host finishes the signaling stream with no answer and logs `every UDP port in --rtc-ports <first>-<last> is taken`. The test shows a command on the same iroh connection still answered (`Rejected(UnknownSession)`), and a new offer succeeding once the holder hangs up. On the client, the closed stream reaches `Signaling::ended` ("the host closed the signaling stream"), which sets `Peer::Lost`. `negotiate` returns `Err` at once, and `Input::Upgraded(Err)` calls `retry_later`. A refusal therefore follows the same 15, 30, 60, 120, 300 s schedule as a failed attempt. It is never retried immediately.

## Backoff

The schedule is a table lookup and its test covers indexes 0 to 5 and 100. The counter resets in two places: `Upgrade::new` when the link connects, and `succeeded` once a subscription over WebRTC succeeds. This change also fixes a small bug. On `main`, `upgraded` called `succeeded()` before subscribing, so a failed WebRTC subscription restarted the backoff from zero. Now it counts as a failure.

**The 15.3 s rejoin is acceptable.** `browser-run.txt` shows `WebRtc again 15286ms after the drop`, which is the 15 s delay plus 0.3 s of negotiation. The client resubscribes on iroh from its cursor at once, so no event is lost. The only cost is relay latency for 15 s.

**A drop after a working upgrade should not retry sooner.** The usual causes of a drop are a phone going to sleep, a network change, or a NAT timeout. In the first two the iroh link usually reconnects as well, and a reconnect makes a new `Upgrade` that attempts at once when the link is live. That leaves the case where iroh survives and WebRTC doesn't. There, a faster first retry saves at most 15 s on the relay, and it needs a second counter or state. A path that flaps after each success retries every 15 s plus its uptime, which is steady and never a hot loop. Keep it as written. Dropping the jitter is also fine: each client's timer starts from its own `Live`, which the dial already spreads out.

## Docs

**PowerShell.** Each command matches Microsoft's documentation. The `New-NetFirewallHyperVRule` line has the shape of the example in both the Hyper-V firewall page and the WSL networking page, with `-Protocol UDP` (an accepted name), `-LocalPorts 50000-50099` (the documented "Port range" form) and `-Action Allow` (the default, so stating it is harmless). `Get-NetFirewallHyperVVMSetting -PolicyStore ActiveStore -Name '{40E0AC32-…}'` is verbatim from the Hyper-V page. `Remove-NetFirewallHyperVRule -Name ConductorWebRTC` uses the documented `ByName` set. The GUID is the one Microsoft gives for WSL. Microsoft doesn't publish the default `DefaultInboundAction`, so "Block" rests on what run 1 observed. The WSL page's advice to set it to `Allow` for inbound connections agrees with that.

**Scope.** The rule applies to WSL's `VMCreatorId`, inbound, UDP, and local ports 50000 to 50099 only. It doesn't narrow `-RemoteAddresses` or `-Profiles`, and both default to Any. On a public Wi-Fi network, anyone who can reach the Windows machine can reach those ports. The risk is small, because the DTLS fingerprint arrives over the authenticated iroh stream. Optional: add `-Profiles Private`, or `-RemoteAddresses` with the LAN's subnet, and say why.

**The same-machine hypothesis is plausible, and correctly labelled untested.** The `.wslconfig` reference says `hostAddressLoopback` is an `[experimental]` setting, `false` by default, that applies only in mirrored mode. Only with it on can the host and the container connect "by an IP address that's assigned to the Host", and `127.0.0.1` always works. The smoke text paraphrases that correctly. The weak point is the 59 datagrams that did arrive. The documentation predicts that traffic fails in both directions, and the hypothesis explains only why the replies fail. The planned experiment settles it either way. If step 3 fixes ICE, the hypothesis holds whatever the reason inbound worked.

## Required fix

1. **The new e2e tests depend on fixed global ports.** `concurrent_webrtc_sessions_take_distinct_ports_in_the_range` assumes 31311 and 31312 are free, and `a_full_range_refuses_the_upgrade_and_iroh_keeps_serving` assumes 31321 is. The failing input is any process holding one of those ports, including the same test running in another checkout. The pair loop runs the implementer's and the reviewer's gates at the same time, and several rounds share this machine (`git worktree list` shows five checkouts now). With UDP `0.0.0.0:31311` held, the first test fails with `left: ([31312], [])` and `right: ([31311], [31312])`. Pick the ports when the test runs. Bind a `std::net::UdpSocket` on `0.0.0.0:0`, read its port `p`, drop the socket, and use `p-p` for the full-range test. For the concurrent test, use two ports found the same way, or `p-(p+1)` with the expectation derived from `p`. Ports from the OS's ephemeral range rarely collide, and never with another copy of this test. The cost is about 20 tokens per test.

## Friction points

1. **`RangeInclusive::new(first, last)` instead of `first..=last`.** Allowed, and the right v7 form for an iterator over integers. The bootstrap review's §A7 kept ranges out because inside `.get(a..b)` they saved only 5% over `split_at_checked`. It never measured a range as an iterator source, and `RangeInclusive::new` is total. `..=` would save 4 tokens and one import here, at one site, far below the bar. No proposal. The checker has a hole, though. `bytes.get(RangeInclusive::new(1, 2))` passes `subset check`, while `bytes.get(1..=2)` is flagged `expr.range` (measured on a scratch crate). That brings indexing by range back under another name. This is for the owner and the tool, not this branch. Either flag a `Range*` constructor as an argument to `get`, `get_mut` or `split_at*`, or record the iterator use in `compromises.md`.
2. **The pass-through arm `Some(socket) => Some(socket)`.** This is the v7 form. `or_else` with a logging closure hides the branch. `if let None` is clippy's `redundant_pattern_matching`. `ok_or(..).inspect_err(..).ok()` is longer and turns an `Option` into a `Result` only to log. The arm costs about 9 tokens. It is one point, and no earlier review logged a point of this kind. No proposal.
3. **`let _ = list.first().ok_or(..)?`.** Not friction. It is the v7 idiom ("`first()` over `is_empty()`"), and `main` already used it here. Both checks are needed, since they give different errors (`NoInterface` against `RangeFull` or `NoSocket`).
4. **The test `match` into `Option` then `.expect`, instead of `assert!(matches!(..))`.** This is the v7 form. §C3 and proposal 0020 keep `assert!` and `matches!` out of tests, and mapping the value and then failing on it follows "assert on values, not conditions". The same goes for `read_frame` returning `Result<Option<T>, _>` with `.unwrap()` in each caller, because errors stay values. One point of each kind across the reviews (none in earlier logs), so no proposal.
5. **`.ok()` inside `filter_map`.** Allowed. "`.ok()` is never a discard" forbids `x.ok();` as a statement that throws a `#[must_use]` result away. Inside `filter_map` it is the filter itself: a line that doesn't parse as a candidate is skipped. Proposal 0009 (`try_into().ok()`) and §A7 (`try_into().ok()?`) use the same conversion.

## Volume and compliance

`subset score` on the six changed files: 22637 tokens to 24980 (+2343), and score 23027.0 to 25380.5.

| File | Tokens before | after | change |
|---|---|---|---|
| `daemon/tests/e2e.rs` | 8128 | 9475 | +1347 |
| `daemon/src/rtc.rs` | 4969 | 5693 | +724 |
| `daemon/src/serve.rs` | 1911 | 2032 | +121 |
| `client/src/upgrade.rs` | 463 | 582 | +119 |
| `daemon/src/main.rs` | 1376 | 1415 | +39 |
| `client/src/link.rs` | 5790 | 5783 | -7 |

Most of it is needed: the range type, its parser and errors (about 300 tokens with the table test), the refusal, and the e2e harness that acts as a browser over raw iroh streams. Moving the log capture into `capture_log` and `captured` shrank the existing stalled-dial test from 302 tokens to 207. Little is avoidable.

- `Conductor::answer` (125 tokens) is a wrapper with one caller. Inlining it saves the six-parameter signature, about 45 tokens, but nests the match one level deeper. Leave it.
- `read_frame` (141 tokens) is the third copy of the frame reader, after `daemon::wire` and `client::wire`. Removing it means making `daemon::wire` public, which is outside this task. Leave it.

The avoidable part is under 2% of the change. The seven qualities hold. `RtcPorts`, `PortRange`, `PortRangeError` and `Unbound` name the states, and every decision is a `match`. Errors are values all the way to the refusal log line. The port-range test and the backoff test are tables of 7 cases each.
