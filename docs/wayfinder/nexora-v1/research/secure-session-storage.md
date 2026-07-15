# Secure recovery and saved-session storage constraints

Research date: 2026-07-15

## Scope and accepted constraints

This note evaluates maintained Linux and Rust mechanisms for Nexora's local session store. The accepted product constraints are:

- recovery records are encrypted and atomically committed;
- an unsaved session is recoverable for seven days;
- a saved session retains data by artifact class, not as one indivisible object;
- the recoverable audio buffer is encrypted and limited to two minutes;
- recovery works after an application or operating-system crash;
- persistent keys use KWallet/Secret Service;
- deletion uses cryptographic erasure and is irreversible within Nexora's key graph;
- Nexora does not replicate session data or storage keys to a cloud service.

The storage threat model is an offline reader of the user's filesystem, including stale database pages. It does **not** protect against root, a malicious process in the same logged-in user session while the wallet is unlocked, plaintext that legitimately exists in Nexora's memory, or data already sent to an AI provider. KDE's own KWallet documentation warns that once a wallet is open its contents can be read by any user process, and the Secret Service specification allows an implementation to unlock objects globally rather than only for one client ([KWallet manual](https://docs.kde.org/stable_kf6/en/kwalletmanager/kwalletmanager/introduction.html), [Secret Service locking](https://specifications.freedesktop.org/secret-service/latest-single/#locking)).

## Recommended decision

Use one local SQLCipher database, with a random raw database key stored in the desktop Secret Service. In addition to SQLCipher page encryption, encrypt every recovery payload and every saved artifact class with an independent application-level AEAD key. Store those independent keys as small Secret Service items; do not derive all artifact keys from a retained per-user or per-session root key.

This double layer is intentional:

1. SQLCipher protects the database schema, indexes, lifecycle metadata, WAL, and current pages at rest.
2. Independent payload keys make it possible to destroy one recovery record or one saved artifact class even when old copies of its ciphertext remain in SQLite pages, WAL history, filesystem snapshots, or SSD remapping.

Use the standard `org.freedesktop.Secret.Service` D-Bus API rather than a Nexora-specific wallet file. The interface was designed jointly by GNOME and KDE developers, and modern KWallet exposes a Secret Service provider while preserving the legacy KWallet API through a compatibility service ([Secret Service introduction](https://specifications.freedesktop.org/secret-service/latest-single/#introduction), [KDE's KWallet transition](https://planet.kde.org/marco-martin-2025-04-14-towards-a-transition-from-kwallet-to-secret-service/)). Never silently fall back to a plaintext key file. If no service is available or the collection remains locked, saving and crash recovery are unavailable; the user may explicitly continue memory-only, but Nexora must say that the session will not be recoverable.

## Facts that constrain the design

### Secret Service is a key store, not a transactional database

Secret Service stores a byte-array secret with a label and lookup attributes. The attributes are not secret and may be stored unencrypted. Collections and items may be locked, the service may prompt for create/delete/unlock, and the service may relock an item at any time. Its API exposes individual create, replace, and delete operations but no transaction spanning multiple items or an external database ([Secret Service items and attributes](https://specifications.freedesktop.org/secret-service/latest-single/#collection-and-items), [prompts](https://specifications.freedesktop.org/secret-service/latest-single/#prompts), [D-Bus API](https://specifications.freedesktop.org/secret-service/latest-single/#dbus-api-reference)).

Consequences:

- store only key material in Secret Service, not transcripts, screenshots, or audio;
- use generic labels and opaque random `key_ref` lookup attributes; never put meeting titles, participants, artifact classes, or session IDs in attributes;
- model every operation that touches both SQLCipher and Secret Service as a resumable two-phase operation;
- do not report deletion complete until the key item is confirmed absent.

KWallet is a suitable Secret Service provider, but the provider remains a security boundary owned by the desktop. KWallet can use an encrypted wallet file or a GPG-backed wallet and can be opened automatically through PAM; these are user/system choices, not properties Nexora can assume ([KWallet storage options](https://docs.kde.org/stable_kf6/en/kwalletmanager/kwalletmanager/introduction.html)).

### SQLite supplies atomic recovery only when configured and used correctly

SQLite records incomplete work in a rollback journal or WAL and automatically uses it to restore a consistent database after a crash. A WAL transaction commits with a commit frame, and the transient WAL index is rebuilt after a crash ([SQLite file format](https://sqlite.org/fileformat.html#the_write_ahead_log)). With WAL and `synchronous=FULL`, SQLite syncs the WAL at every commit and documents the mode as ACID across operating-system crashes and power loss; `synchronous=NORMAL` can lose the most recent committed transactions after either event ([SQLite `synchronous`](https://sqlite.org/pragma.html#pragma_synchronous)).

SQLite still relies on correct filesystem locking, sync, and storage behavior. WAL must remain beside the database, and copying or deleting a hot WAL breaks recovery. Network filesystems are particularly risky because their locking may not match SQLite's assumptions ([SQLite corruption guidance](https://www.sqlite.org/howtocorrupt.html)). Nexora should require a local filesystem for this store and must never rename, copy, back up, or unlink an open database.

For a standalone file, `rename(2)` atomically replaces the pathname, but durability additionally requires syncing the new file and its containing directory. Linux documents that `fsync(file)` alone does not guarantee that the directory entry reached storage ([rename(2)](https://man7.org/linux/man-pages/man2/rename.2.html), [fsync(2)](https://man7.org/linux/man-pages/man2/fsync.2.html)). Nexora should use SQLite transactions for recovery records instead of implementing a custom temp-file protocol.

### SQLCipher is maintained and available from Rust

SQLCipher encrypts database pages with AES-256-CBC, uses a random IV per page, and authenticates each encrypted page and IV with HMAC-SHA512. It accepts a raw 256-bit key, avoiding a password KDF when the key already comes from a wallet ([SQLCipher design](https://www.zetetic.net/sqlcipher/design/), [raw key API](https://www.zetetic.net/sqlcipher/sqlcipher-api/#example-2-raw-key-data-without-key-derivation)). `rusqlite` supports system and bundled SQLCipher builds through explicit Cargo features ([rusqlite repository](https://github.com/rusqlite/rusqlite#optional-features)).

As of this research date, SQLCipher 4.17.0 uses SQLite 3.53.3 and includes fixes beyond the SQLite 3.51.3 WAL-reset corruption fix. The dependency must be pinned and updated deliberately rather than relying on an arbitrary distribution library ([SQLCipher 4.17.0 release](https://www.zetetic.net/blog/2026/07/08/sqlcipher-4.17.0-release/), [SQLCipher 4.14.0 WAL note](https://www.zetetic.net/blog/2026/03/17/sqlcipher-4.14.0-release/)).

### File overwrite is not secure deletion on an SSD

NIST SP 800-88 Rev. 2 says SSD overprovisioning and wear levelling make it infeasible for an application using normal writes to address every location that held sensitive data. It recommends purge or destroy rather than relying on overwrites for stronger assurance. Cryptographic erase works by sanitizing the target encryption key or the wrapping key that makes it accessible, but all copies of the target key must be sanitizable and backups or escrowed keys must be handled separately ([NIST SP 800-88r2, sections 3.1.1 and 3.2](https://doi.org/10.6028/NIST.SP.800-88r2)).

Secret Service specifies logical `Delete()` behavior, not physical zeroization of an implementation's wallet file, and it does not mandate any particular master-password mechanism ([Secret Service `Delete`](https://specifications.freedesktop.org/secret-service/latest-single/#org.freedesktop.Secret.Item.Delete), [out-of-scope mechanisms](https://specifications.freedesktop.org/secret-service/latest-single/#whats-not-included)). Therefore the defensible product claim is:

> Deletion makes the selected data permanently inaccessible to Nexora by deleting its only application-accessible decryption key. Nexora does not overwrite SSD blocks and cannot remove copies made by system backups, filesystem snapshots, or a compromised keyring provider.

Do not claim certified media sanitization or guaranteed physical erasure on every Linux system.

## Storage and key model

Place the database at `$XDG_DATA_HOME/nexora/sessions.db` (default `~/.local/share/nexora/sessions.db`). Saved sessions are user data, and keeping recovery and saved rows in one database makes promotion to `Saved` a single SQL transaction. The XDG specification defines `XDG_DATA_HOME` for user-specific data and says missing application directories should be created with mode `0700` ([XDG Base Directory Specification](https://specifications.freedesktop.org/basedir/)). Create the directory as `0700`, the database and auxiliary files as `0600`, and use a process umask of `0077` while creating them.

Recommended key graph:

| Key | Persistence | Protects | Erasure boundary |
| --- | --- | --- | --- |
| `K_db` | one Secret Service item | the complete SQLCipher database, WAL, schema, and indexes | removing all Nexora local session storage |
| `K_recovery(session)` | one item per ephemeral/recoverable session | versioned recovery snapshot, task ledger, and recovery-only text/binary artifacts | discard, successful save promotion, or seven-day expiry |
| `K_saved(session, class)` | one item per retained artifact class | only that saved class, such as transcript, translation, assistant output, summary, screenshots, audio, or metadata snapshot | deleting that class or the whole saved session |
| `K_audio_checkpoint(session)` | one replaceable item per active audio ring | the oldest still-live audio chain key and its sequence number | ending/discarding the buffer; advancing it gives forward erasure of older chunks |

The database maps an opaque random `key_ref` to each encrypted blob. Except for the well-known `K_db` lookup entry, Secret Service sees only a generic Nexora label and random identifier. Direct independent class keys are preferable to wrapping every class key with a long-lived session root: stale copies of a wrapped class key would remain decryptable for as long as that root survives.

Use an authenticated-encryption algorithm for every payload. A practical maintained Rust choice is XChaCha20-Poly1305 with a fresh random 192-bit nonce; the RustCrypto implementation is pure Rust, supports the extended nonce, and has received an independent audit ([RustCrypto `chacha20poly1305`](https://github.com/RustCrypto/AEADs/tree/master/chacha20poly1305)). Authenticate at least `format_version`, `session_id`, `artifact_class`, `generation`, `key_ref`, and chunk sequence/timestamps as associated data so ciphertext cannot be silently moved between records. Hold keys in `zeroize`-on-drop containers; RustCrypto's `zeroize` uses volatile writes and memory fences so the compiler does not optimize the clearing away ([RustCrypto `zeroize`](https://github.com/RustCrypto/utils/tree/master/zeroize)). Memory zeroization reduces accidental remnants but is not a defense against a live privileged process, swap, hibernation, or a prior core dump.

Suggested logical tables are:

- `sessions`: opaque ID, lifecycle, creation/expiry times, current generation, and operation state;
- `encrypted_blobs`: session, artifact class, generation, key reference, nonce, ciphertext, and AEAD format version;
- `audio_chunks`: sequence and timing metadata plus nonce and ciphertext;
- `key_operations`: a durable queue for create, replace, and delete reconciliation;
- `storage_meta`: schema version, crypto format version, and minimum compatible application version.

Only non-sensitive identifiers and lifecycle fields should be queryable columns. Meeting titles, participants, prompt contents, source names, and provider responses belong inside encrypted blobs.

## Recovery lifecycle and crash behavior

### Creating and checkpointing

Secret Service and SQLite cannot participate in one atomic transaction, so use this order for a new recoverable session:

1. Generate `K_recovery` with the operating-system CSPRNG and create its Secret Service item.
2. Begin one SQLCipher transaction, insert the `sessions` row, encrypted generation 1, and a completed key-operation marker, then commit.
3. If the process dies after step 1, startup garbage collection sees an app-tagged key with no committed database reference and deletes it. If it dies after step 2, both the key and record exist.

Each subsequent checkpoint writes a complete, versioned snapshot as a new generation and changes `sessions.current_generation` in the **same** transaction. Readers only use the referenced committed generation. Old generations may be collected later; they remain harmless after `K_recovery` is deleted. Use WAL with `synchronous=FULL`, keep every connection on the same settings, and serialize writes on a dedicated storage worker rather than GTK's main thread.

Checkpoint on every meaningful session-state transition and at a short bounded interval while capture/transcription changes recoverable content. A checkpoint cadence is a product latency/performance decision; crash tests should establish it rather than assuming every token or audio frame requires an `fsync`.

On startup, before showing recoverable sessions:

1. unlock/read the wallet and key the SQLCipher connection;
2. verify that the connection is encrypted (`PRAGMA cipher_status`) and let SQLite recover the hot WAL;
3. resume incomplete `key_operations` in idempotent order;
4. cryptographically erase expired records without decrypting or listing them;
5. authenticate the latest committed snapshot and map a session that crashed while capturing to the product's interrupted/paused recovery state; never reactivate capture sources automatically.

SQLCipher documents `cipher_status` as the check that a connection is actually keyed and `cipher_integrity_check` as an HMAC verification of every page ([SQLCipher utilities](https://www.zetetic.net/sqlcipher/sqlcipher-api/#utilities)). Run the cheap status/key check on every open and the full integrity check after an unclean shutdown, migration, or explicit diagnostics, not necessarily on every launch if its measured cost is high.

### Seven-day expiry

Set `expires_at = created_at + 7 days` for an unsaved recovery record. Enforce expiry in the resident process, from a user-level timer while Nexora is not resident, and before listing records on every launch/resume. A powered-off machine cannot execute deletion at the exact instant; the first code path after boot must process expiry before recovery UI or decryption.

Secret Service has no item-TTL operation in its specified D-Bus API, and a locked collection cannot be modified. Consequently, an exact wall-clock guarantee that the key is physically absent at day seven is impossible while the machine is powered off or the wallet remains unavailable ([Secret Service D-Bus API](https://specifications.freedesktop.org/secret-service/latest-single/#dbus-api-reference), [locked items](https://specifications.freedesktop.org/secret-service/latest-single/#locking)). Nexora can guarantee that it will not offer or decrypt an expired record and that it will delete the key at the first executable, unlocked opportunity. If the requirement means key absence at the exact instant regardless of code execution and wallet availability, it is not implementable with KWallet/Secret Service alone.

Expiry uses the same deletion state machine as explicit discard:

1. atomically mark the session `deleting` and remove it from all user-visible/recoverable queries;
2. delete `K_recovery` and `K_audio_checkpoint`, retrying prompts/locked-service failures;
3. confirm each key is absent;
4. purge ciphertext rows and leave only a non-sensitive tombstone if audit/debugging requires it.

A crash at any point resumes from `deleting`. Until step 3 succeeds, the status is `DeletionPending`, not `Deleted`. Expiry must never become a reason to extend recovery silently because the wallet was locked.

### Promotion to saved data by artifact class

Saving is not a lifecycle flag on the recovery ciphertext. It is a re-encryption boundary:

1. create independent `K_saved(session, class)` items only for the artifact classes selected by the user;
2. decrypt the final recovery representation in memory and re-encrypt each selected class with its class key;
3. commit all saved class blobs and the `saving_committed` lifecycle state in one SQL transaction;
4. delete `K_recovery` and the recovery audio checkpoint, confirm deletion, then mark `Saved` and purge recovery rows;
5. delete orphan saved-class keys if the SQL transaction never committed.

This prevents a stale recovery page from retaining transcript, screenshots, or audio classes that the user did not elect to save. Deleting a single saved class follows the same `deleting -> key absent -> ciphertext purge` protocol without affecting other class keys.

The storage layer should use a closed artifact-class enum, while the GTK settings expose the classes independently. A reasonable initial taxonomy is `transcript`, `translation`, `assistant_output`, `summary`, `screenshots`, `audio`, and `session_metadata`; final names and defaults are product decisions. Screenshots and recorded audio should remain explicit opt-ins.

## Two-minute encrypted audio ring

Write audio as small independently authenticated chunks, never as a plaintext temporary file. A forward-only key chain avoids placing dozens of durable chunk keys in Secret Service:

1. `K_chunk(i) = HKDF(K_chain(i), "nexora/audio/chunk")`;
2. encrypt chunk `i` with a fresh nonce and authenticate session ID, sequence, exact capture interval, codec, and format version;
3. `K_chain(i+1) = HKDF(K_chain(i), "nexora/audio/next")`, then zeroize the prior in-memory chain value;
4. persist only the chain key for the oldest chunk still inside the two-minute window and its sequence in `K_audio_checkpoint`.

For each slide of the window, first commit the new chunk and removal of expired catalog rows in SQLCipher, then replace the Secret Service checkpoint with the new oldest chain value. That ordering preserves crash recovery: a crash before the wallet update leaves an older chain key that can still derive every live chunk. Record the pending checkpoint advance in `key_operations`; after wallet replacement is confirmed, older chunks are no longer derivable through Nexora's current key graph.

The database query and transaction must enforce a maximum interval of 120 seconds, trimming a boundary chunk if needed rather than treating “N chunks” as two minutes. If audio is not a selected saved class, delete its checkpoint key during finalization/discard. If audio is selected for saving, re-encrypt the retained audio artifact under `K_saved(session, audio)` before deleting the recovery chain key.

An important limitation remains: Secret Service item replacement is not specified to zero old backend storage. The ratchet provides forward erasure at the application/key-graph level, not a certified claim that a forensic attacker can never recover a historical wallet block.

## Migration and key rotation

Maintain separate `schema_version` and `crypto_format_version` values. A new application must refuse to modify a database from a newer unknown version and should offer a clear read-only/export recovery path only when it can do so without plaintext temporary files.

- Run ordinary schema changes and `PRAGMA user_version` updates in one SQLite transaction.
- Keep payload AEAD decoders versioned. Re-encrypt one artifact class at a time with a newly created class key and the same two-phase key-operation protocol; never mutate ciphertext in place without a durable rollback/retry state.
- For SQLCipher major-format changes using default historical settings, `PRAGMA cipher_migrate` supports an in-place upgrade. For custom settings or a side-by-side encrypted migration, `sqlcipher_export()` copies schema and data to an attached database with a specified key; it does not copy `user_version`, so Nexora must set and verify that explicitly ([SQLCipher migration API](https://www.zetetic.net/sqlcipher/sqlcipher-api/#migration-and-compatibility)).
- A side-by-side migration must keep both databases encrypted, close all connections before the final same-filesystem swap, validate `cipher_status`, `cipher_integrity_check`, schema version, and row counts, then sync the new file and parent directory. Retain the old key only while rollback is possible; once the new store is committed, queue deletion of obsolete keys and unlink the old encrypted file.
- Never create a plaintext migration database or an automatic decrypted backup. Database backups, home-directory snapshots, and keyring backups are outside Nexora's “no cloud” guarantee and can invalidate strong cryptographic-erasure claims, as NIST's CE guidance notes.

## Rust implementation fit

The maintained pure-Rust `secret-service` crate talks to Secret Service through `zbus`, supports Tokio and RustCrypto, and exposes collection/item create, search, get, set, and delete operations. Its project describes the API as stable and feature-complete ([secret-service-rs](https://github.com/open-source-cooperative/secret-service-rs)). It fits Nexora's existing Tokio runtime and gives enough control to use encrypted D-Bus sessions and opaque attributes. The higher-level `keyring` ecosystem is also maintained and has explicit Secret Service stores, but its maintainers recommend depending on `keyring-core` plus the exact store implementation when an application needs control over backend selection ([keyring-rs](https://github.com/open-source-cooperative/keyring-rs)).

Recommended stack to prototype and benchmark:

- `secret-service` with the Tokio/RustCrypto feature, using Diffie-Hellman transfer rather than the protocol's plaintext transfer mode;
- `rusqlite` on one dedicated worker thread, linked to a pinned current SQLCipher release;
- RustCrypto `chacha20poly1305`, `hkdf`, `sha2`, `zeroize`, and an OS-backed random source for payload encryption and the audio ratchet;
- `serde` with an explicitly versioned recovery envelope; reject unknown required fields or versions rather than partially restoring them.

No network API is needed for this subsystem. Nexora should reject or relocate a database path on a network filesystem, never sync the database itself, and never upload keys, recovery records, or saved artifacts. This does not prevent a user-controlled backup tool from copying `$XDG_DATA_HOME` or the desktop wallet, so the privacy UI and deletion documentation must say so.

## Verification required before implementation is accepted

1. Crash-inject at every boundary before/after Secret Service create/replace/delete and before/after SQLite commit; verify that startup reconciliation produces either the prior complete state or the new complete state, never a plaintext or half-visible record.
2. Kill the process, kill the user service, reboot, and simulate power loss with WAL + `synchronous=FULL`; verify the latest acknowledged checkpoint and automatic interrupted/paused recovery.
3. Test GNOME Keyring and current Plasma/KWallet with locked, unlocked, prompt-cancelled, unavailable, and relocked collections. Confirm that no fallback key file is created.
4. Fill the audio ring through many wraparounds and crashes. Verify authenticated recovery of at most 120 seconds and failure to decrypt pre-window chunks using only the current checkpoint.
5. Save every combination of artifact classes, then inspect raw database, WAL, temp files, logs, and keyring attributes for plaintext or identifying metadata.
6. Delete one class, one recovery session, and all storage. Confirm key absence before reporting success, including after a crash during deletion.
7. Exercise schema, payload-format, and SQLCipher migrations using only encrypted intermediate files; verify rollback and orphan-key cleanup.
8. Benchmark wallet operations, `fsync` checkpoint cadence, full cipher integrity checks, and audio-ratchet updates off GTK's main thread.

## Bottom line

SQLCipher plus independent per-recovery/per-artifact AEAD keys in Secret Service is the smallest design that simultaneously gives Nexora crash-atomic records, seven-day recoverability, class-level saved retention, and meaningful cryptographic erasure. SQLite alone cannot erase stale pages, and one long-lived session key cannot erase one artifact class. Secret Service alone cannot atomically coordinate with a database, so durable operation states and idempotent reconciliation are mandatory. Physical secure deletion on arbitrary SSDs and keyring implementations remains unverifiable; the product must promise key-graph irreversibility, not block overwriting or certified media sanitization.
