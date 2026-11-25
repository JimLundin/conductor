# Fleet pairing and recovery

A standalone host still pairs with its own ticket. Once a host is in a
fleet it requires a fleet device certificate, including on connections
from apps that paired with its former ticket.

## Set up a fleet

1. Stop the first host. Use its current relay URL to run
   `orchestrator fleet --data-dir <dir> init --name <name> --relay <url> --backup <file>`.
   Enter a passphrase when prompted and keep the sealed file separately.
   Start the host again.
2. In the installed app, choose **Pair with a fleet**. Run the displayed
   `orchestrator pair` command on the root host and paste the signed reply
   into the app that generated the request. The device's private key
   stays in IndexedDB and is non-extractable. The browser needs secure
   WebCrypto with Ed25519 support.
3. In **Add host**, request a fleet invitation and run the displayed
   `orchestrator join <ticket>` on the machine that is joining. Use the same
   data directory for `join` and the subsequent `host`. An invitation
   expires after an hour and can be retried by the same host identity
   during that window.

App and daemon must use matching protocol versions. The sidebar discovers
hosts through the signed public roster. Host-to-host synchronization
carries the full signed configuration, which is encrypted locally and
released only after an iroh peer presents a valid Host certificate.

Enter the shareable API keys or selected Claude setup token on the root
host's Credentials screen. Provider sign-ins that write their own login
files remain local. The app can show updated instances when the new host
applies the signed configuration. The real CLI sign-in assumptions and
the second-host Home Screen walkthrough are still to be verified.

## Availability and revocation

The root must be online to invite hosts and edit shared credentials.
A member that has already synchronized can keep serving while the root
is unavailable. On a new daemon start, a member waits for authenticated
fleet synchronization before accepting browser sessions. The peer does
not have to be the root. Hosts track only the latest valid configuration
they have received.

`orchestrator fleet status` shows the local signed configuration version.
On the root, stop the daemon and run `orchestrator fleet revoke <node-id>`
to revoke a host or device, then restart it. Other hosts enforce the
revocation after synchronization. Rotate shared API keys and the Claude
token after revoking a host that received them. Sign out the revoked
host's local ChatGPT login from OpenAI account settings if applicable.

## Restore the root

The init backup preserves the signing key and the original root iroh
identity. Stop the old root and keep it stopped before recovering that
identity. Use a surviving member with the highest available configuration
version to run `orchestrator fleet export-configuration --output <file>`.
Use the backup passphrase to seal the export.

On the replacement, run
`orchestrator fleet --data-dir <new-dir> restore --backup <init-file> --configuration <export>`.
The restore verifies the signed export with the backed-up root key before
using it. `--initial-only` is for a fleet whose configuration never
changed. If a version was lost with the root before another member
received it, the surviving members cannot reconstruct it.

## Enroll from a standalone host or approve another browser

In **Add host**, paste a running standalone host's join link into
**Standalone host join link**, give the host a name and choose **Admit
standalone host**. The root verifies the join link directly with the
standalone host. The admitted daemon switches to certificate
authentication and closes its existing token-authenticated connections.

To pair a second browser, paste the request displayed by that browser
into **New browser pairing request** on an already authenticated app.
Choose **Approve new browser** and paste the signed reply back into the
browser that generated the request. The root signs the reply while it is
online.

## Update provider CLIs

CLI versions and platform SHA-256 digests are part of each Conductor
release. Update the compiled pins together with protocol support and
recorded fixtures, then deploy that tested release to each host. Each host
downloads and verifies its pinned binaries when needed. An explicit
`--cli <provider>=<path>` remains available for tests and development.

Choose a session's model in the app or set the host's default with
`--model <provider>=<model>`. Instance-specific preferences still take
precedence over host defaults. Codex analytics stays disabled. Fleet
configuration carries membership and shared credentials; it does not
select CLI versions or publish provider settings.

## Fleet format changes

This release uses `conductor-fleet/4` and new signing and encryption
contexts for full configurations, local fleet state and root backups.
Hosts must support the same fleet protocol version to synchronize.
Certificates and the public roster are now signed over their JSON bytes
and carried as those bytes, so earlier certificates no longer verify.

Previous `fleet.sealed` files and root backups cannot be opened by this
release. A host with unreadable fleet state refuses to start; it does not
fall back to standalone ticket authentication. Stop the affected hosts,
set the old fleet state and backups aside, and initialize and enroll the
fleet again with fresh backups. Pair the apps again with the new root,
which has a new signing key. There is no conversion of old state.
