# Credential store backup and recovery

The credentials in `<data>/credentials` are sealed under a 32-byte store
key. Conductor chooses where that key lives once, when it creates the
store, and keeps it there:

- **The OS keystore**, for the OS user that runs the daemon or
  `conductord login` (use the same account for both). macOS uses that
  user's default Keychain, Windows that user's Credential Manager with
  local-machine persistence, and Linux an unlocked default Secret Service
  collection on the user's session bus. The entry is labelled `Conductor
  credential store`; its account is the random identifier in
  `<data>/store.key.slot` (on Windows, part of the target name; on Linux,
  the `store` attribute). It holds 32 binary bytes.
- **The private file `<data>/store.key`**, 64 hexadecimal digits readable
  by the owner only, when no keystore is usable as the store is created
  (a headless Linux or WSL host often has no Secret Service) or when an
  empty `<data>/store.key.file` asks for it. The host log reports the
  file fallback.

A store never moves between the two. To change where the key lives, set
the data directory's `credentials`, `store.key` and `store.key.slot`
aside and enter the credentials again.

## Backing up

For a keystore store, back up the OS vault and keep `store.key.slot`
with the data directory backup: the slot names the entry. For a file
store, back up `store.key` to private storage apart from ordinary data
backups. Either key alone cannot decrypt anything without
`credentials`, and `credentials` cannot be decrypted without its key.

## Recovering access

- If the keystore is locked or unreachable, run Conductor under the
  original user with the vault unlocked. On Linux, make sure the daemon
  can reach the right session bus and collection; a daemon started by a
  system service or another login may see a different vault. Conductor
  does not fall back to a file for a store whose key is in the keystore.
- If the keystore entry was deleted, restore it from the vault backup
  under the account in `store.key.slot`, with its original value.
- If the key is lost, the sealed credentials cannot be decrypted. Conductor
  refuses to create a new key beside existing `credentials`; set the
  credential files aside and enter the credentials again.

The keystore protects the key at rest under the OS account's security
boundary. On a host using the file, host permissions protect the key, and
encryption protects copies of `credentials` that travel without it.
