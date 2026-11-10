# S3a notes: Dioxus web and iroh, native and in the browser

Observed on 2026-11-10 in WSL2 (Ubuntu 26.04) with iroh 1.4.1, iroh-mdns-address-lookup
0.6.1, Dioxus and dx 0.7.10, and Playwright 1.65.0 with Chromium headless shell 158.
Each fact says how it was observed.

## Reproduce

- `tools/check.sh` runs fmt, clippy (native, and wasm32 for `proto`, `client` and `app`) and
  the tests.
- `tools/s3-native.sh` runs `conductord s3-host` and the headless client twice, and
  writes `docs/s3/native-run.txt`.
- `tools/s3-browser.sh` builds the web app, serves it under `/conductor/`, and drives
  Chromium and WebKit with `crates/app/tests-e2e/browser.mjs`. It writes
  `docs/s3/browser-run.txt` and `docs/s3/iphone-390x844.png`. Its WebRTC scenarios are
  described under "WebRTC upgrade for the browser" below.

## iroh 1.4 API names actually used

Read from the iroh-1.4.1 sources in `~/.cargo/registry` and confirmed by the code compiling
and the runs above.

- **Endpoint and relays.** `Endpoint::builder(presets::N0)` sets n0's production relays
  (`endpoint::default_relay_mode()`), pkarr publishing and resolving, DNS lookup outside
  browsers, and the ring crypto provider. A relay map from config would go through
  `Builder::relay_mode(RelayMode::custom([url, ...]))`. The e2e tests use
  `presets::Minimal` with `RelayMode::Disabled` so they run on loopback without network.
- **Waiting for the relay.** `Endpoint::online().await`, then
  `endpoint.addr().relay_urls()` gives the home relay for the ticket. The host printed
  `https://euc1-1.relay.n0.iroh.link./` in every run.
- **Renames since 0.x.** Node ids are `EndpointId` (`iroh_base::PublicKey`), addresses are
  `EndpointAddr::new(id).with_relay_url(url)`, and discovery is now called address lookup.
- **Local-network discovery.** mDNS is not in the `iroh` crate in 1.x. It is the separate
  crate `iroh-mdns-address-lookup`, added with
  `Builder::address_lookup(MdnsAddressLookup::builder())`. The host and the native client
  both enable it. The desktop app's debug log shows `swarm_discovery` sending and receiving
  `_irohv1._udp.local.` records.
- **Accepting.** `iroh::protocol::Router::builder(endpoint).accept(ALPN, handler).spawn()`
  with a `ProtocolHandler` whose `accept(&self, Connection)` is an `async fn`.
- **Path detection.** `Connection::paths_stream()` yields a `PathList` snapshot whenever the
  open or selected path changes. The client takes the path with `Path::is_selected()` and
  maps `is_ip()` to `Direct` and `is_relay()` to `Relayed`. `Unknown` is only shown before
  the first snapshot arrives. `Connection::path_events()` exists for per-path events and
  is not used.

## WASM

- **iroh features in the browser.** `iroh` with `default-features = false,
  features = ["tls-ring"]` builds for `wasm32-unknown-unknown`. This matches n0's
  `browser-echo` example. Without `tls-ring` there is no crypto provider for the `N0`
  preset.
- **ring needs clang for wasm32.** The first wasm build failed in `ring v0.17.14`'s build
  script with `CC_wasm32-unknown-unknown = None`. There is no sudo here, so LLVM 21 was
  unpacked from the Ubuntu debs with `apt-get download` and `dpkg-deb -x` into
  `~/.local/opt/clang21`, with `clang` and `llvm-ar` wrappers in `~/.local/bin`. GitHub's
  `ubuntu-latest` image ships clang, so the Pages workflow needs nothing extra. Every
  developer machine that builds the web app needs clang. This also applies to the Windows
  CI build later.
- **getrandom.** `getrandom 0.4` with the `wasm_js` feature was enough. No
  `--cfg getrandom_backend` RUSTFLAGS were needed, unlike `browser-echo`'s
  `.cargo/config.toml`. Observed from the wasm build and the browser run succeeding without
  it.
- **Runtime-agnostic async.** The client uses `n0_future::task::spawn` and
  `n0_future::time::sleep` (wasm-bindgen-futures in the browser, tokio natively) and
  `tokio` with only `sync` and `macros` for channels and `select!`. Both compile for
  wasm32. The same `HostLink` code ran under Dioxus web, Dioxus desktop and plain tokio.
- **No COOP/COEP headers needed.** `python3 -m http.server` sends neither header and the
  browser run passed. This settles the §13.1 [verify] for GitHub Pages.
- **Bundle size.** Observed with `stat` and `gzip -9` on the `dx build --platform web
  --release` output (dx runs wasm-opt):
  - Default release profile (opt-level 3, no LTO): **5,631,463 bytes raw, 1,791,412 bytes
    gzip** in the run recorded in `browser-run.txt`. The JS glue is about 73 KB.
  - With `opt-level = "z"`, `lto = true` and `codegen-units = 1`, set through
    `CARGO_PROFILE_RELEASE_*` for a one-off build: **4,045,641 bytes raw, 1,485,467
    bytes gzip**. This is not committed, because it would also slow the daemon's release
    build. A separate wasm profile is the follow-up.
  - Not measured: how much of this is iroh and rustls versus Dioxus. Use `twiggy` before
    acting on it.

## Connect times

Each range covers the three runs made of each script, so treat it as an order of magnitude.

- **Native headless to host, same machine** (`docs/s3/native-run.txt`): Live 0.33 to 0.37
  s after process start. That covers endpoint bind, relay connect, QUIC handshake, Hello and
  Subscribe. After a forced reconnect, Live came 8 to 18 ms after the drop, because a
  requested reconnect skips the backoff. The path was `Direct` in both cases, but host and client
  were on one machine, so this says nothing about hole punching across NATs.
- **Browser, through the relay** (`docs/s3/browser-run.txt`): Live 952 to 1,215 ms
  after navigation on first open. That includes loading and instantiating 5.6 MB of wasm from
  localhost, binding the endpoint and connecting to the relay. After a reload without the
  fragment, Live came in 854 to 961 ms, reconnecting from the ticket in local storage. The path was
  `Relayed`. An in-page reconnect without a reload was not timed in the browser.
- **Desktop app under WSLg** (log of `target/dx/app/debug/linux/app`): from `Connecting` to
  `Live` took 625 ms, then the path became `Direct`.

## Desktop on Linux

- `dx build --platform desktop` compiles with webkit2gtk-4.1 2.52.6 and libsoup-3.0 3.6.6.
  Observed from the build. The client core runs natively with native iroh and mDNS.
- Launched for 25 s under WSLg (`DISPLAY=:0`) with `CONDUCTOR_TICKET` set, against a
  running `conductord s3-host`. It **reached Live, then reported path `Direct`**. Observed
  from the app's `link status` log lines and the host's `client connected ...
  client="desktop"` and `subscribed after_seq=0`. No screenshot, because no screen-capture
  tool is installed. The window was not driven; S3b owns that.

## Problems and surprises

- **ring and clang** (above). This is the one build-environment requirement S3 added.
- **clippy and rsx.** Nightly clippy flags every `"{text}"` node in `rsx!` as
  `useless_format`. `crates/app/src/main.rs` allows that lint crate-wide.
- **dx index.html.** dx 0.7.10 uses `crates/app/index.html` as the page template. Observed
  in the built `index.html`. It appends the configured title inside an existing `<title>`,
  so the template's title must stay empty. `public/` is copied verbatim to the output root.
  `base_path = "conductor"` rewrites the script URLs to `/conductor/assets/...`, while the
  template's relative `manifest.json` and icon links resolve under `/conductor/` too.
- **Endpoint drop.** iroh logs `ERROR Endpoint dropped without calling Endpoint::close` when a
  process exits without closing it. The headless client closes it; the desktop app does
  not yet, so killing it logs the error.
- **mDNS log noise.** `swarm_discovery` logs `INFO ... no addresses for peer` at startup.
  The daemon's default filter sets it to `warn`.
- **The prompt is not in the transcript.** The S3 `EventKind` list has no user message, so
  the UI shows replies, approvals and results but not what was asked. P1's `UserMessage`
  (§5.2) fixes this.
- **Not verified in a real browser on a phone.** Chromium at 390x844 with touch proves
  layout, tap target sizes (44 px measured) and the relayed path. It cannot show iOS
  Safari's keyboard behavior, safe areas, or Add to Home Screen. Those need the iPhone
  (S3b and P3).

## WebRTC upgrade for the browser

Observed on 2026-11-10 in the same WSL2 setup (default NAT, eth0 172.23.130.230) with
str0m 0.25.0, Chromium headless shell 158 and Playwright's WebKit 27.2. The runs are in
`docs/s3/browser-run.txt`. Architecture §5.4 describes the design as built.

### Reproduce

- `tools/s3-browser.sh` runs three scenarios in Chromium and WebKit, each against its own
  `conductord s3-host` in a temporary data dir. `upgrade` reaches Live on the relay, waits
  at most 10 s for `WebRtc`, then runs a full turn, a reload and a fresh-install pairing.
  `fallback` starts the host with `--test-rtc-blackhole`, so every WebRTC datagram is
  dropped. `drop` starts it with `--test-rtc-drop-after 8`, so the host resets all data
  channels after its eighth frame, in the middle of the turn's deltas.
- Each scenario checks the transcript text, the approval, the message and the "Turn
  finished" count against literal values, so a duplicate or missing event fails it. The
  script then checks the host log for which transport carried each command and
  subscription.

### Host stack: str0m, not webrtc-rs

str0m 0.25 was chosen. The reasons, from reading both crates' sources:

- str0m is sans-IO with no internal tasks. The driver in `crates/daemon/src/rtc.rs` owns
  the sockets and the loop, so socket choice, logging and test knobs are ordinary code.
  Its `rust-crypto` feature is pure Rust, which avoids aws-lc's C build on the Windows
  host later.
- `webrtc` 0.21 is a recent rewrite as an async layer over the sans-IO `rtc` crates. It
  pulls in `smol`, `async-*` and `quinn-udp` next to tokio, and its gatherer does STUN
  for you. Vibe Kanban's `relay-webrtc` has since moved from webrtc-rs to str0m as well
  (it calls `Rtc::direct_api()`).
- The cost of str0m was STUN. It does no gathering, and its `StunMessage::parse` refuses a
  Binding success without MESSAGE-INTEGRITY, which ICE needs and public STUN servers never
  send. The first build therefore never learned its srflx address. Encoding the RFC 8489
  request and the XOR-MAPPED-ADDRESS reply took about 60 lines plus a test.
- str0m cannot parse the browsers' mDNS host candidates (`<uuid>.local`). The host logs
  and skips them. The browser's checks still arrive, and str0m learns the address as a
  peer-reflexive candidate.

### Timings

Measured on the page's clock, from the first `Live` on the relay to the first `Live` on
`WebRtc`, over the two script runs of the day:

| Engine | First open | After reload | Drop scenario |
|---|---|---|---|
| Chromium | 263, 264 ms | 115, 135 ms | 179, 293 ms |
| WebKit | 141, 311 ms | 136, 164 ms | 176, 349 ms |

The host logs DTLS up 140 to 250 ms after it accepts the offer. After a drop, the client
was back on the relay before the next delta. It was on `WebRtc` again 1.1 to 2.3 s later,
because the first retry waits 1 to 2 s. In `fallback` the client gives up after 15 s and
retries after 1 to 2 s, then 2 to 4 s, doubling to 2 min. Both turns completed on the
relay while those attempts ran.

### Candidates seen locally

- The browsers gathered one mDNS host candidate and one srflx candidate,
  203.0.113.1 (the home router).
- The host offers one host candidate per up, non-loopback IPv4 interface, here eth0
  172.23.130.230 and docker0 172.17.0.1, each on its own ephemeral port. STUN gave one srflx
  per socket, on 203.0.113.1. Google and Cloudflare reported the same public port for
  a given socket, so the WSL NAT and the router mapped it the same for both destinations.
  That is the property a phone on another network needs.
- The selected pairs, as the browser's `getStats` reports them (browser side, host side),
  were `host/host` and `prflx/host`. WebKit's checks arrived from both 172.23.130.230 and
  10.255.255.254, WSL's loopback alias. Neither run proves anything about NAT traversal,
  because browser and host share one machine.

### Size

The release wasm went from 5,650,545 bytes raw and 1,797,576 gzip (the S3a run) to
5,739,104 raw and 1,824,992 gzip. That is 88,559 bytes raw and 27,416 gzip for the
`web-sys` WebRTC bindings and the client code.

### Problems and surprises

- **Data channels cannot half-close.** A command's client side does not finish its
  channel. The host closes it after it writes the result, and the result still arrives
  first.
- **A dropped connection is only noticed fast if the host resets the channels.** Ending
  the str0m session alone left the browser waiting for ICE consent to expire. The drop
  knob therefore closes every channel first.
- **str0m logs `WARN Drop BufferedAmountLow for id: N`** when a channel closes with an
  event pending. It is harmless.
- **Native connect times in this run were slower than in S3a.** `native-run.txt` shows Live
  2.7 to 5 s after start, against 0.34 s before. The scripts now give each run a new host key
  in a temporary data dir. With a reused key, two runs were Live in about 1.1 s. The native
  client does not touch the WebRTC code, so the likely cause is lookup of a key that was
  never published. That is a guess and was not measured further.
- **Not done:** IPv6 candidates (WSL's NAT mode has none), browser send backpressure
  (`bufferedAmount` is ignored, which is fine for S3's traffic), and a native Rust test of
  the host's WebRTC path. Only the browser runs cover it.

### What the phone test should show

On the iPhone, the status line should change from `Relayed` to `WebRTC <browser>/<host>`
within a few seconds of Live. `srflx` or `prflx` on the browser side with `srflx` on the
host side means the path crosses both NATs directly. If it stays `Relayed`, these host log
lines diagnose it, all at `info` or `warn` with target `daemon::rtc`:

- `webrtc offer accepted` lists the host's candidates and the resolved STUN servers.
- `webrtc STUN answer` gives each socket's public address. A warning that STUN servers saw
  different public addresses means the NAT maps per destination, and a direct path from
  mobile data will not work. `webrtc STUN server did not answer` means UDP to STUN is
  blocked.
- `webrtc remote candidate` gives each candidate the phone sent, with its type and
  address. A phone on mobile data should send `srflx` (and possibly IPv6, which the host
  does not offer).
- `webrtc datagrams arriving` appears once the first check from the phone reaches the
  host. If it never appears, the phone's packets do not get through the router and WSL's
  NAT. The fix is in §5.4's remedies, starting with `networkingMode=mirrored`.
- `webrtc client gave up` carries the phone's own reason: ICE and connection states, and
  what each side gathered.
- `webrtc session ended` gives the reason, the duration, the datagram counts in and out,
  `connected` and the last remote address.
