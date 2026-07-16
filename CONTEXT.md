# Nexora Domain

Nexora provides private, contextual assistance during live conversations while keeping capture, processing, and disclosure under the user's control.

## Language

**Session**:
A bounded period that groups active meeting inputs and the context, guidance, notes, and usage derived from them. It is independent of whether the resident Nexora interface is visible or interactive.
_Avoid_: Meeting, recording, app runtime

**Source Authorization**:
The user's revocable permission for Nexora to use a specific input source. Authorization does not mean the source is currently being captured.
_Avoid_: Enabled source, active source

**Source Activation**:
The actual capture of an authorized input source during a session. Activation ends immediately when its authorization is revoked.
_Avoid_: Permission, authorization

**Paused Session**:
A session that retains its existing context while all source activation and new AI processing are stopped. Hiding the Nexora interface does not pause a session.
_Avoid_: Hidden session, ended session

**Ephemeral Session**:
A session that does not remain in the user's history after normal completion but retains temporary recovery state while active. An interrupted ephemeral session can be restored from its latest checkpoint.
_Avoid_: Unsaved session, memory-only session

**Recovery Record**:
The temporary representation used to restore an interrupted ephemeral session. It is removed after normal completion or when its recovery period expires.
_Avoid_: Saved session, backup, history entry

**Saved Session**:
A session retained in the user's history according to explicit artifact-specific retention choices. Enabling retention later cannot recreate raw audio or images that were already discarded.
_Avoid_: Recovery record, ephemeral session

**Pending Deletion**:
Saved session data marked for removal when the current session completes. The pending removal remains visible to the user until it is executed or cancelled.
_Avoid_: Deleted data, hidden data

**Degraded Session**:
An active session in which an authorized source or processor is unavailable while the remaining pipeline continues. Missing intervals are explicit, and recovery returns the same session to normal operation.
_Avoid_: Failed session, paused session

**Recovery Buffer**:
A bounded, temporary copy of source data retained only to bridge a processor interruption. Processing success or expiry removes the buffered data without changing its disclosure permissions.
_Avoid_: Saved audio, recovery record, session history

**Safe Share**:
A verified full-monitor share source that omits every Nexora surface and reveals the normally composed content behind it. Loss of that guarantee stops live video rather than exposing an unverified frame.
_Avoid_: Screen exclusion, black-box redaction, ordinary monitor capture

**Processor**:
A session capability that transforms authorized source data or existing context into derived information. A processor is distinct from the model or provider selected to perform it.
_Avoid_: Provider, model, source

**Finalizing Session**:
A session whose sources have stopped while pending source data and end-of-session processors complete. Retention or deletion has not yet been applied.
_Avoid_: Active session, completed session

**Session Review**:
The finalized transcript, notes, usage, failures, and summary presented before the user chooses to save or discard the session.
_Avoid_: Saved session, recovery record

**Pending Review**:
A session review awaiting an explicit save or discard choice while no sources remain active. It is recoverable for a limited period and defaults to discard on expiry.
_Avoid_: Saved session, active session

**Disclosure Grant**:
The user's explicit permission for a particular provider to receive a class of session data within a profile. A configured credential or fallback does not itself grant disclosure.
_Avoid_: API key, source authorization, provider configuration

**Abandoned Task**:
An AI task whose local result is no longer accepted after cancellation when remote termination cannot be guaranteed. Its already disclosed input and provider-side cost cannot be revoked.
_Avoid_: Failed task, completed task
