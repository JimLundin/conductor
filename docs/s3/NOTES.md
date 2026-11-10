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
  Chromium with `crates/app/tests-e2e/browser.mjs`. It writes `docs/s3/browser-run.txt`
  and `docs/s3/iphone-390x844.png`.

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
