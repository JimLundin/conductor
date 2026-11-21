# Credential store backup and recovery

Conductor chooses the keystore for the OS user that runs the daemon or
`conductord login`. Use the same account for both. macOS uses that user's
default Keychain, Windows uses that user's Credential Manager with
local-machine persistence, and Linux uses an unlocked default Secret
Service collection on the user's session bus. For example, a headless
Linux or WSL host may have no accessible Secret Service. A store that has
yet to migrate uses the private `store.key` file there. The headless
fallback is reported in the host log.

## Moving an existing installation to its native keystore

1. Stop the daemon and any `conductord login` process. Back up the data
   directory and the original `store.key` to private storage independent
   of the host. Keep a copy of the key separate from ordinary data backups.
2. Upgrade and start Conductor under the account that will run it. On
   Linux, start it with access to the unlocked Secret Service session.
   Conductor imports the existing key, reads it back, then records a
   random keyring account identifier in `store.key.slot` and a key
   fingerprint in `store.key.native`. The slot is created before the
   import so an interrupted import can find the same entry. The
   encrypted credentials and pairing keys remain intact.
3. On the next successful open, Conductor verifies the marker and the
   original native key again before removing the leftover `store.key`.
   Verify that the expected instances are present and that the plaintext
   file is gone. Avoid starting an older daemon against this migrated
   directory. To roll back, restore the matching backup and key first.

Only the native entry contains the decryption key after migration.
`store.key.slot` identifies the item in the OS vault. The vault entry is
labelled `Conductor credential store` and contains the slot as the
account, as part of the target name on Windows, or as the `store`
attribute on Linux. Protect the OS vault backup and preserve both
non-secret marker files alongside the data directory backup. A vault
entry contains 32 binary bytes. A `store.key` backup holds the same 32
bytes encoded as 64 hexadecimal digits.

## Recovering access

- If a previously usable keyring is unavailable or locked, run Conductor
  under the original user with the original vault unlocked. On Linux,
  ensure the daemon has access to the right session bus and collection.
  A daemon started by a system service or a different login may see a
  different vault.
- If the native entry was deleted, restore it from the OS vault backup
  under the original slot with its original value. The daemon compares the
  recovered value with the fingerprint in `store.key.native`. If a
  surviving plaintext `store.key` disagrees with the native entry, both
  copies are preserved for inspection.
- To start without the original vault when you have the original
  `store.key` backup, stop all Conductor processes. Back up the current
  data directory and install the original key as `store.key` with mode
  0600 on Unix or a user-only ACL on Windows. Leave the migration markers
  in place, then create an empty `store.key.file` to select file storage.
  Conductor checks the restored key against the recorded fingerprint
  before it opens the store. Remove the file-storage marker once the
  original vault is accessible again. The marker is also available for a
  fresh installation that deliberately runs without a native vault.
- If you have neither the original key nor a backup of the OS vault, the
  sealed credentials cannot be decrypted. Preserve the old directory
  and credentials for recovery. You can configure new credentials in a
  fresh data directory. Do not create a replacement key for encrypted
  files you still need.

The keyring protects the key at rest under the OS account's security
boundary. On a host using the file fallback, host permissions protect
the key and encryption protects copies of the credentials file that
travel separately from it.
