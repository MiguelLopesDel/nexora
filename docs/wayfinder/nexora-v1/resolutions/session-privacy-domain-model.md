# Resolution: Define the Live Session and Privacy Domain Model

Resolved with the user on 2026-07-15.

## Lifecycle

Nexora may remain resident without a session. V1 permits one active, paused, or finalizing session at a time. Hiding or making the overlay non-interactive does not change the session.

The lifecycle is `Active ↔ Paused → Finalizing → Review → Saved | Discarded | Pending Review`. A pending review is recoverable for seven days by default, configurable from one to thirty days, and expires to `Discarded`. Abrupt failure creates an interrupted session: recovery returns a formerly capturing session to `Paused`, while finalization and review resume without activating sources. `Degraded` is an explicit condition on a continuing session, not a separate lifecycle state.

## Authorization and Disclosure

Source authorization is separate from source activation. Operating-system permission belongs to Nexora, while automatic activation is granted per profile and specific source. Imported or duplicated profiles do not inherit automatic activation consent, credentials, or sensitive disclosure permissions.

A provider credential does not authorize disclosure. A Disclosure Grant permits a specific provider within a profile to receive particular data classes: derived text, raw audio, original images, voice signatures, or saved history. Fallbacks remain within these grants.

## Recovery and Retention

An ephemeral session maintains an encrypted Recovery Record but does not enter permanent history after normal completion. The record contains derived session state, not raw audio or images unless their retention was explicitly enabled. Interrupted records remain recoverable for seven days by default and are never cloud-synchronized.

Raw audio and images are discarded after successful processing unless retained explicitly. Enabling save mid-session preserves existing derived state but cannot recreate already discarded raw artifacts. Disabling save schedules deletion at completion, displays a persistent warning, and remains reversible until the final confirmation. Completion performs irreversible cryptographic deletion without using a trash location.

A configurable, encrypted Recovery Buffer retains up to two minutes of audio per source during a transcription outage. Successful processing or expiry removes the buffered data, and remote disclosure still requires an existing grant.

## Failure and Cancellation

A source failure degrades the session, records an explicit gap, and retries only the same authorized source. It never substitutes another device silently. Processor failures are isolated; only the affected capability stops, and only pre-authorized fallbacks may run. Historical reprocessing requires user action.

Safe Share fails closed. It briefly freezes the last verified clean frame and, after three seconds, emits a neutral unavailable frame instead of falling back to unsafe monitor capture. Capture resumes only after the clean-source guarantee is validated again.

Cancellation stops local work and requests remote cancellation where supported. If remote termination cannot be guaranteed, the task becomes Abandoned: later output is ignored and not retained, already disclosed input cannot be revoked, provider billing may continue, and no retry or fallback runs.

## Finalization

Finalization stops every source immediately, drains pending input, and runs end-of-session processors. Review presents transcript, notes, summary, usage, failures, and actions before the user chooses save or discard. Closing review creates a Pending Review and does not block a new live session.
