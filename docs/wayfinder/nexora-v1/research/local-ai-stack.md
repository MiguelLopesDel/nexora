# Local audio, transcription, diarization, translation, vision, and OCR stack

Research date: 2026-07-15

## Scope and interpretation

This note recommends a Linux-first local inference stack for Nexora. In this
document, **remote diarization** means diarizing the remote/call-audio track,
not sending audio to a remote service. Cloud providers can remain optional
fallbacks, but they are not required by the proposed default path.

Claims linked to project documentation, source repositories, model cards, or
specifications are facts. Sections explicitly labelled **Recommendation** or
**Inference** are product/architecture judgments. No cross-project benchmark
number is used to choose a winner; Nexora still needs task-specific benchmarks
on its supported hardware.

## Executive recommendation

| Layer | Recommended v1 choice | Why | Important boundary |
| --- | --- | --- | --- |
| Audio graph | Two independent PipeWire capture streams through `pipewire-rs` | Native Linux graph access, explicit device/application targeting, and source identity survives downstream processing | Do not mix the microphone and call audio before attribution |
| VAD | Silero VAD, preferably the GGML model already supported by `whisper.cpp` | Small, CPU-friendly, MIT-licensed, and configurable endpointing | Run an independent VAD state machine per track |
| Live local ASR | `whisper.cpp` with multilingual Whisper models | Native C/C++, MIT code and weights, bounded integration surface, CPU plus CUDA/ROCm/Vulkan/OpenVINO options | Whisper is windowed, not truly streaming; partial text is revisable |
| Remote-side speaker attribution | Label the separate track as `Remote` live; optionally refine the remote track asynchronously with `sherpa-onnx` diarization | Source separation gives reliable local-vs-remote attribution immediately; sherpa has local ONNX and Rust APIs | Do not promise stable individual remote-speaker labels in the live path until measured |
| Translation | Stable transcript segments into a local translation provider; Ollama first, CTranslate2/Marian as the dedicated future backend | Reuses Nexora's OpenAI-compatible provider path initially; dedicated translation can later reduce resource use | Whisper's translation task targets English only and `turbo` does not perform it |
| Screen description | Ollama native/OpenAI-compatible API with an explicitly installed vision model; start evaluation with Qwen3-VL 2B/4B/8B Instruct variants | Existing provider shape, local image API, model management, broad Linux hardware support | A VLM description is generative and must not replace faithful OCR |
| OCR | Tesseract 5 with explicit language packs | Mature CPU-native C++ API, UTF-8, 100+ languages, Apache-2.0 | Benchmark screen UI text; keep PaddleOCR as an optional quality escalation |

The important architectural seam is a provider-neutral pipeline:

```text
PipeWire mic ----------> resample/mono -> per-track VAD -> ASR worker --+
                                                                     +--> timestamped transcript
PipeWire call audio ---> resample/mono -> per-track VAD -> ASR worker --+        |
              |                                                               +--> translation
              +--> optional remote-track diarization -------------------------+

Portal screenshot ---> crop/redact ---> OCR ----------------------------------> exact text lane
                               +-----> Ollama vision --------------------------> descriptive lane
```

Keep inference off GTK's main thread. Use bounded queues and cancellation:
obsolete partial-ASR and screen-analysis jobs may be cancelled, while finalized
transcript segments and late responses can be marked delayed and retained in
history.

## 1. Audio capture and track separation

### Facts

PipeWire represents devices and application streams as graph nodes. WirePlumber
documents `Audio/Source` for capture devices, `Audio/Sink` for playback devices,
`Stream/Input/Audio` for application capture streams, and
`Stream/Output/Audio` for application playback streams. A capture stream can
set `target.object` to a chosen node, including another stream node, instead of
using the default device. See the [WirePlumber linking policy](https://pipewire.pages.freedesktop.org/wireplumber/policies/linking.html).

PipeWire also exposes `stream.capture.sink`, which requests capture from a
sink's monitor ports. The official [audio capture example](https://docs.pipewire.org/1.2/audio-capture_8c-example.html)
shows both `PW_KEY_TARGET_OBJECT` and `PW_KEY_STREAM_CAPTURE_SINK`. This is the
mechanism for a whole-output monitor. Targeting a communication application's
playback stream is more selective when that node is visible and linkable.

The maintained [`pipewire` Rust bindings](https://pipewire.pages.freedesktop.org/pipewire-rs/pipewire/)
provide registry, node, stream, and buffer APIs. The bindings note that most
PipeWire objects are not thread-safe and recommend a dedicated main loop with
channels to other threads. Registry events report nodes appearing and
disappearing, which is necessary for call-app restarts and device hotplug.

The XDG ScreenCast portal returns selected screen/window **video** PipeWire
streams and documents no audio-selection option in `SelectSources`; inspect the
[ScreenCast portal interface](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html).
It should continue to authorize visual capture, but it is not the API for call
audio. This conclusion is an inference from the portal's documented options.

PipeWire's [echo-cancel module](https://docs.pipewire.org/page_module_echo_cancel.html)
can expose an echo-cancelled microphone source using the playback reference.
It also supports monitor mode. This can reduce remote speech leaking back into
the microphone when speakers are used, but it does not identify remote
speakers.

### Recommendation

Create two capture streams and preserve their identity through the entire
pipeline:

1. `Local`: the user-selected microphone `Audio/Source` (optionally an existing
   echo-cancelled virtual source).
2. `Remote`: either the selected call application's `Stream/Output/Audio` or a
   selected output sink monitor.

Use a PipeWire registry listener to populate a UI selector from properties such
as `object.serial`, `node.name`, `node.description`, `application.name`,
`application.id`, `media.role`, and `media.class`. Node IDs are ephemeral. Use
the serial to target the currently live object, but persist a composite of
descriptive/application properties to reacquire a node after recreation (a new
node receives a new serial). Surface a visible disconnected state rather than
silently falling back to another source. The portal's newer guidance to prefer
`object.serial` over reusable node IDs for a live video session reinforces the
same lifecycle concern, although audio selection itself is outside that portal.

Normalize each downstream ASR branch to 16 kHz mono PCM while retaining a
monotonic capture timestamp and original track ID. Never infer `You` versus
`Remote` by voice clustering when the graph already provides that distinction.
If the fallback is a sink monitor, warn that notification sounds and every
other application routed to that sink may be included.

Run capture callbacks as real-time-safe producers only: copy/reference the
minimum buffer data into a bounded ring and return the PipeWire buffer. Resample,
VAD, encoding, persistence, and network access belong on worker threads.

### UI/configuration required

- Capture mode: `Call application`, `Output device monitor`, or `Microphone only`.
- Separate microphone and remote-source selectors, with live level meters and
  a test action.
- Explicit recording/transcription consent, active-capture indicator, and a
  per-session start/stop control.
- Reconnect policy: wait for the same source, ask again, or stop; no unannounced
  fallback.
- Optional echo-cancelled microphone selector/toggle with a plain-language
  explanation.
- Source diagnostics: node description, application, sample rate/channels,
  connection state, and fallback reason.

## 2. Voice activity detection and endpointing

### Facts

[Silero VAD](https://github.com/snakers4/silero-vad) publishes JIT and ONNX
models under MIT, supports 8 kHz and 16 kHz audio, and documents a roughly
two-megabyte JIT model. Its own performance page says a 30+ ms chunk takes less
than 1 ms on one CPU thread under its test conditions; that is a first-party
measurement, not a Nexora latency guarantee. The repository includes ONNX,
C++, and Rust examples.

`whisper.cpp` now directly supports a converted Silero VAD model and exposes
threshold, minimum speech/silence duration, maximum speech duration, padding,
and overlap controls in its [VAD documentation](https://github.com/ggml-org/whisper.cpp#voice-activity-detection-vad).

### Recommendation

Use Silero on CPU for both tracks, each with independent recurrent state and
endpoint timers. A single detector over a mixed signal would corrupt turn
boundaries and speaker attribution. Feed only detected speech plus configurable
pre/post padding to ASR, but keep a short ring buffer so leading phonemes are
not lost.

Expose a simple preset (`Responsive`, `Balanced`, `Noise tolerant`) and hide raw
thresholds behind an Advanced expander. Preset values must be selected from
Nexora's own noisy-room and conferencing fixtures; this research does not
invent them. Long uninterrupted speech needs a maximum-utterance split with
overlap so one speaker cannot monopolize the inference queue.

Record endpoint reason (`silence`, `max duration`, `manual stop`, `source lost`)
with each utterance. This makes latency and truncation failures diagnosable.

## 3. Local streaming transcription

### Runtime comparison

#### `whisper.cpp` — recommended default

The official [`whisper.cpp` repository](https://github.com/ggml-org/whisper.cpp)
is MIT-licensed and documents Linux, CPU-only inference, integer quantization,
CUDA, AMD ROCm, cross-vendor Vulkan, and OpenVINO acceleration for the encoder
on Intel CPUs and GPUs. It has a C API and a benchmark tool. Its published
unquantized model table is useful for install planning, not a promise of peak
process memory in Nexora:

| Family | Model file | Documented memory |
| --- | ---: | ---: |
| tiny | 75 MiB | about 273 MB |
| base | 142 MiB | about 388 MB |
| small | 466 MiB | about 852 MB |
| medium | 1.5 GiB | about 2.1 GB |
| large | 2.9 GiB | about 3.9 GB |

Quantized files can use less disk and memory; quality and speed effects depend
on model, quantization, and hardware and therefore need Nexora measurements.

[OpenAI Whisper](https://github.com/openai/whisper) code and weights are MIT.
It provides multilingual recognition and speech-to-English translation. The
reference implementation processes audio with a sliding 30-second window; its
`turbo` model returns the original language even when translation is requested.

`whisper.cpp` includes a [`whisper-stream` example](https://github.com/ggml-org/whisper.cpp/tree/master/examples/stream)
that samples the microphone repeatedly and calls itself a naive real-time
example. Therefore “streaming Whisper” means repeated/windowed decoding and
hypothesis stabilization, not a native incremental decoder.

#### `faster-whisper` / CTranslate2 — useful alternative, not v1 default

[`faster-whisper`](https://github.com/SYSTRAN/faster-whisper) is an MIT
reimplementation on CTranslate2 with CPU INT8 and NVIDIA CUDA paths, Python as
its primary API, batched transcription, and project-published benchmarks.
It is attractive for a Python sidecar and NVIDIA deployments. It is not the v1
default because Nexora is a small Rust desktop binary and needs first-class
AMD/Intel options without a Python environment. Do not transplant its published
benchmark numbers into Nexora; its own table fixes model, beam size, CPU/GPU,
versions, and batch configuration.

#### `sherpa-onnx` — true-streaming candidate and diarization companion

[`sherpa-onnx`](https://github.com/k2-fsa/sherpa-onnx) is Apache-2.0, runs local
streaming and non-streaming ASR plus VAD and diarization on Linux, and publishes
C and Rust APIs. It supports multiple model families, including streaming
Zipformer/Paraformer-style models and non-streaming Whisper. This is the right
candidate if Nexora later needs a genuinely streaming decoder. Model coverage,
accuracy, punctuation, language support, artifact license, and binary size must
be evaluated per chosen model; the runtime license does not automatically
license every downloaded model.

### Recommendation

Load one `whisper.cpp` model per session and use a bounded inference worker
rather than independently loading a model for each track. Preserve track ID,
utterance ID, source time range, and revision number on every result. Suggested
state semantics:

- `partial`: may be replaced; never persist as a final transcript row.
- `final`: VAD endpoint reached and a final pass completed.
- `corrected`: a later pass changed a previously finalized segment; preserve
  audit/revision metadata.
- `late`: useful but arrived after the UI's actionable latency window; archive
  it without displacing current guidance.

Prioritize final remote utterances for reply suggestions, but do not starve the
local track. Cancel superseded partial decodes and cap queued utterances by
count and captured duration. If the queue exceeds the configured latency
budget, retain audio only when the user opted into recording; otherwise mark a
gap explicitly instead of silently inventing continuity.

Start evaluation with multilingual `base` as a low-resource candidate and
multilingual `small` as a balanced candidate. This is a candidate set, not a
quality conclusion. English-only `.en` variants should appear only when the
session language is fixed to English. A first-run/local benchmark should report
measured real-time factor, cold load time, time to first partial, finalization
delay, peak RSS, and peak VRAM for the user's selected model/backend.

### Integration shape

Prefer a narrow internal `SpeechRecognizer` interface independent of the FFI:

```text
load(model, backend, threads) -> capabilities
begin_utterance(id, track, language_hint)
push_pcm(id, timestamp, samples)
finish_utterance(id) -> stream<TranscriptRevision>
cancel(id)
health() -> load/backend/memory/queue state
```

Whether implemented with direct C FFI or a pinned Rust wrapper, isolate unsafe
ownership and model lifecycle in one module. Do not expose GGML types to the
conversation domain.

## 4. Diarization of the remote track

### What track separation already solves

Two PipeWire tracks deterministically distinguish the Nexora user from all
remote participants. Live transcript labels should therefore start as `You`
and `Remote`, which is more reliable and cheaper than diarizing a mixed signal.
Individual remote attendees still share one playback track and require
clustering if per-person labels are desired.

### Local options

[`sherpa-onnx` speaker diarization](https://k2-fsa.github.io/sherpa/onnx/speaker-diarization/index.html)
provides segmentation plus embedding models and examples for Rust and other
APIs. It is the best integration-shape candidate because the same native ONNX
runtime can later cover true-streaming ASR. The documentation presents
file/segment pipelines rather than promising stable online clustering, so v1
should treat it as asynchronous refinement.

[`pyannote.audio`](https://github.com/pyannote/pyannote-audio) is an MIT
Python/PyTorch toolkit. Its `community-1` weights run locally, accept 16 kHz
mono, support CPU and CUDA, and offer “exclusive” diarization intended to make
alignment with transcript timestamps easier. The
[`community-1` model card](https://huggingface.co/pyannote/speaker-diarization-community-1)
licenses weights under CC-BY-4.0 and requires accepting access conditions and
using a Hugging Face token for the initial download. It can run offline after
download. This gating, Python/PyTorch footprint, attribution requirement, and
lack of documented AMD/Intel GPU paths make it an opt-in quality fallback, not
a silently bundled default.

### Recommendation

For v1 live behavior, show `Remote` without unstable A/B identities. Run
`sherpa-onnx` only on finalized remote audio in a lower-priority worker, then
map its time intervals onto final ASR word/segment timestamps. Speaker labels
are session-scoped (`Remote A`, `Remote B`) and may be revised until the session
is finalized. Never infer real names without an explicit user mapping.

Before enabling this by default, benchmark sherpa's published model combinations
against representative Nexora calls. Audit and record the license and checksum
of every segmentation and embedding artifact. If the quality gate fails, offer
`pyannote community-1` as an explicitly installed sidecar/profile and satisfy
CC-BY attribution in the model manager and notices.

The UI needs `Off`, `Remote track only`, and `Post-session refine` modes; expected
speaker count/range; model status; and an action to rename session-local labels.
Overlapping remote speech must remain representable as overlap/uncertain rather
than forcing a confident single speaker.

## 5. Local translation

### Facts and options

Whisper can translate speech to English, but it is not an arbitrary
source/target text translator, and its `turbo` model does not honor the translate
task. Nexora needs a downstream text translation layer for targets such as
Brazilian Portuguese.

Ollama can run a chosen multilingual text model through the provider interface
Nexora already uses. This minimizes initial integration work but a generative
LLM consumes more memory, can paraphrase, and may contend with the vision model.
Model behavior and licensing are model-specific.

[`CTranslate2`](https://github.com/OpenNMT/CTranslate2) is an MIT C++/Python
runtime for encoder-decoder families including Marian/OPUS-MT, M2M-100, NLLB,
mBART, and T5. It supports x86-64 and ARM64 CPUs, quantization, CUDA, and a
source-build HIP backend. It is a strong dedicated future backend, but every
translation model has its own license and supported language directions. In
particular, do not assume a permissive runtime license makes NLLB or any OPUS
checkpoint suitable for every distribution/commercial use.

### Recommendation

Ship translation as a provider-neutral asynchronous stage after final ASR:

1. v1 local path: an explicitly selected Ollama text model with a prompt that
   requests faithful translation and no commentary.
2. Future compact path: CTranslate2 with separately packaged, audited Marian or
   OPUS-MT language-pair models.

Preserve and display the source transcript beside the translation. Translation
failure or lateness must never block final transcript persistence. Cache by
source segment revision, model, and target language; invalidate when ASR corrects
the source. UI fields: enabled, target locale, provider/model, glossary/prompt,
maximum delay, and whether late translations are archived.

## 6. Local screen description with Ollama

### Facts

Ollama's [vision API](https://docs.ollama.com/capabilities/vision) accepts images
with text; its REST API uses base64 image data. Its
[OpenAI compatibility endpoint](https://docs.ollama.com/api/openai-compatibility)
supports vision chat requests, so Nexora's current OpenAI-compatible provider
can carry a screenshot. The native API additionally exposes model listing,
details, pull progress, loaded models, and lifecycle controls that are useful
for settings/model management.

The Ollama server repository is MIT, but model licenses are independent. The
official [Qwen3-VL 4B model card](https://huggingface.co/Qwen/Qwen3-VL-4B-Instruct)
is Apache-2.0 and describes image understanding and GUI-element recognition.
Ollama's [Qwen3-VL registry](https://ollama.com/library/qwen3-vl/tags) currently
offers 2B, 4B, and 8B Instruct artifacts among larger variants. Artifact sizes
on that mutable registry are download sizes, not total runtime memory or a
quality ranking.

Ollama documents local CPU execution, NVIDIA CUDA, AMD ROCm, and Vulkan on
Windows/Linux (including an Intel Linux driver path) in its
[hardware support](https://docs.ollama.com/gpu). It also documents that context
length and parallel requests increase memory use, models remain loaded for a
configurable `keep_alive`, and insufficient memory causes requests/models to
queue in the [FAQ](https://docs.ollama.com/faq).

### Recommendation

Treat Ollama as an external local daemon reached at `127.0.0.1` by default; do
not silently install, start, expose, or upgrade it. Use the existing compatible
chat path for inference and a small native Ollama management adapter for health,
model inventory/details, pulls initiated by the user, pull progress, loaded
processor/memory state, and unload.

Evaluate Qwen3-VL Instruct at 2B, 4B, and 8B quantizations on the Nexora screen
tasks. The 2B/4B variants are reasonable low-resource candidates and 8B a
quality candidate, but this is an inference from size—not a benchmark result.
Do not select a default until the screen-description/OCR fixture suite is run.

Keep the default endpoint loopback-only and reject non-loopback “local” endpoints
unless the user explicitly enables network providers. Offer guidance for
Ollama's local-only mode (`OLLAMA_NO_CLOUD=1`) but do not claim Nexora can verify
an external daemon's process environment. Never auto-pull a multi-gigabyte model;
show exact registry-reported download size, license, storage path, checksum when
available, and cancellable progress first.

Screen analysis should be event-driven/manual. Capture once through the portal,
crop/redact according to the user's selection, then make one bounded request.
Avoid polling because it consumes compute and repeatedly processes sensitive
screen contents. Return capture timestamp and crop/source metadata with every
answer so a late result is visibly attached to the correct screen state.

## 7. OCR

### Tesseract — recommended baseline

[Tesseract](https://github.com/tesseract-ocr/tesseract) is Apache-2.0, exposes a
C/C++ API, supports UTF-8 and more than 100 languages, and can emit plain text,
hOCR, TSV, ALTO, and other formats. Linux distributions package the engine and
language data separately; the [installation documentation](https://tesseract-ocr.github.io/tessdoc/Installation.html)
describes over 130 language and 35 script packages in distro repositories.
Its current LSTM engine is CPU-oriented and image preprocessing materially
affects recognition quality.

Tesseract is the v1 baseline because it adds no Python or GPU runtime and returns
text plus boxes/confidence suitable for screen-aware prompts. Bundle no language
pack without recording its exact license/version. Let users install/select only
the packs they need.

### PaddleOCR — optional escalation

[`PaddleOCR`](https://github.com/PaddlePaddle/PaddleOCR) is Apache-2.0 and
provides multilingual OCR/document pipelines, but brings a substantially larger
Paddle/Python/deployment surface. Evaluate it only if Tesseract fails Nexora's
small-font, mixed-language, or layout acceptance gates. It should be a separate
optional backend/sidecar, not part of the resident overlay's baseline footprint.

### Recommendation

Run OCR and vision as distinct lanes:

- OCR output is the faithful, selectable text representation with boxes and
  confidence; low-confidence spans stay marked uncertain.
- VLM output is a description/interpretation that may hallucinate and must be
  labelled as AI-generated.
- For “explain this screen,” feed cropped image plus OCR text to the selected
  VLM when resource budget permits. For “copy/translate text,” prefer OCR and a
  text translator without invoking a VLM.

Expose OCR language packs, auto/fixed script mode, crop, scaling/preprocessing
preset, confidence threshold, and whether recognized text is persisted.

## 8. Hardware and backend policy

| Hardware | ASR (`whisper.cpp`) | Ollama vision/text | VAD/OCR/diarization | v1 policy |
| --- | --- | --- | --- | --- |
| CPU x86-64/ARM64 | Native CPU; quantized models available | CPU fallback | Silero and Tesseract on CPU; sherpa-onnx CPU baseline | Always-supported fallback; expose thread cap |
| NVIDIA | CUDA backend | CUDA on documented supported GPUs/drivers | Keep VAD/OCR on CPU; sherpa/pyannote CUDA only if separately enabled | Prefer CUDA when self-test succeeds |
| AMD | ROCm or Vulkan | ROCm for listed GPUs; Vulkan broadens support | Keep VAD/OCR/diarization on CPU initially | Prefer ROCm for a documented GPU, otherwise Vulkan; show fallback |
| Intel GPU | OpenVINO encoder or Vulkan | Vulkan with documented Intel Linux driver path | Keep VAD/OCR/diarization on CPU initially | Benchmark OpenVINO vs Vulkan locally; never claim full Whisper runs in OpenVINO when docs specify encoder |
| Intel/other NPU | No general v1 commitment | No general v1 commitment | sherpa supports some named embedded NPUs, not a generic desktop NPU promise | Report unsupported; do not silently use experimental paths |

“Auto” must be an observable policy, not a mystery setting. At startup/self-test,
record runtime version, detected devices, selected backend, model offload, driver
error, and fallback reason. Never claim GPU acceleration merely because a GPU
exists. Ollama's loaded-model API/`ollama ps` reports CPU/GPU split; surface that
state when available.

GPU memory is shared across ASR, diarization, translation, and vision. Serialize
or priority-schedule heavy work by default: live ASR first, user-triggered screen
analysis second, translation next, post-session diarization last. Offer model
unload/keep-alive controls. Ollama documents that parallelism multiplies context
allocation and can queue requests, so Nexora should set its own lower bounded
queue and timeout rather than relying on the daemon's much larger default queue.

## 9. Settings and model-management contract

The GTK settings surface needs enough information to make privacy, cost, and
resource use opt-in and explainable.

### Audio

- Microphone and remote source, capture mode, levels, reconnect policy.
- Capture consent/indicator, start/stop, and raw-audio retention (off by default).
- Per-track health and recent endpoint reason.

### Speech

- Engine, model family/quantization, fixed/automatic language, backend/device,
  CPU thread cap, and one-click local benchmark.
- VAD preset plus advanced threshold, silence, maximum utterance, padding, and
  overlap.
- Partial-result visibility, actionable latency budget, queue limit, and late
  result history policy.

### Diarization

- Off/remote-only/post-session mode, expected speaker range, model/backend,
  install/license status, and session-label rename.

### Translation

- Off/on, target locale, provider/model, glossary/prompt, maximum delay, and
  source-plus-translation display.

### Vision and OCR

- Separate vision provider/model and OCR backend/language packs.
- Manual preset/capture source, crop/redaction preview, persistence, timeout,
  and maximum concurrent requests.

### Model manager and diagnostics

- Runtime health/version, installed versus available models, exact artifact
  source, version/revision, checksum, license, required attribution, download
  size, local path, and delete action.
- User-initiated cancellable download with progress; partial-download cleanup.
- Cold/warm load state, measured RSS/VRAM, chosen processor/backend, queue depth,
  last error, and fallback reason.
- Refuse or clearly warn on unknown/non-commercial/gated model licenses. Runtime
  and model licenses must be stored separately.

## 10. Latency/resource validation plan

Published runtime benchmarks are not interchangeable. Nexora should check in a
reproducible fixture manifest (not user recordings) and record:

- Audio capture: dropped frames, clock discontinuities, reconnect time, and
  cross-track leakage.
- VAD: missed speech, false triggers, clipped starts/ends, and endpoint delay.
- ASR: WER/CER, time to first partial, finalization delay, real-time factor,
  correction rate, cold/warm model load, peak RSS/VRAM, and queue age.
- Diarization: DER, speaker-count error, overlap handling, label churn, and
  timestamp-to-transcript alignment.
- Translation: adequacy/faithfulness human rubric, unwanted commentary rate,
  latency, and source-revision invalidation.
- OCR: word/character accuracy, box overlap, confidence calibration, latency,
  and peak memory on UI screenshots at several scale factors.
- Vision: task-specific human rubric for screen description, grounding to OCR,
  hallucination rate, time to first/final token, and memory.

Cover at least English and Brazilian Portuguese, accented speech, code and
product names, quiet/noisy rooms, headphones/speakers, one and multiple remote
participants, overlapping speech, notification sounds, 100–200% display scale,
light/dark themes, terminal text, browser text, and mixed-language screens.

Acceptance values belong in the implementation ticket after measuring current
hardware tiers. The selection gate should compare candidates on the same audio,
prompts, model precision/quantization, and warm/cold conditions. Record runtime,
model, driver, and OS versions with every report.

## 11. Risks and explicit non-goals

- Per-application PipeWire nodes are ephemeral and policy-dependent. Whole-sink
  monitor is a necessary fallback, but may capture unrelated system audio.
- Speaker playback can leak remote voices into the microphone. Separate graph
  tracks do not remove acoustic echo; headphones or an echo-cancelled source
  remain important.
- Whisper partials can rewrite earlier text. The UI must distinguish partial,
  final, corrected, and late states.
- Real-time per-person diarization is not promised for v1. `You` versus `Remote`
  is reliable from source tracks; Remote A/B is asynchronous and revisable.
- No model is downloaded, no microphone/call audio is captured, and no recurring
  screen capture starts without explicit user action/configuration.
- “Local” means a loopback/runtime path selected by the user. A configurable
  LAN URL or cloud-enabled daemon must not be presented as equivalent privacy.
- No blanket commercial-use statement applies to a runtime's model catalog.
  Verify every model artifact and attribution obligation independently.
- NVIDIA, AMD, Intel GPU, or NPU presence is not evidence of acceleration; only
  a successful backend self-test and runtime-reported processor state are.

## Primary sources

### Linux media

- [PipeWire audio capture example](https://docs.pipewire.org/1.2/audio-capture_8c-example.html)
- [PipeWire key names](https://docs.pipewire.org/group__pw__keys.html)
- [PipeWire echo cancel](https://docs.pipewire.org/page_module_echo_cancel.html)
- [WirePlumber linking policy](https://pipewire.pages.freedesktop.org/wireplumber/policies/linking.html)
- [`pipewire-rs` documentation](https://pipewire.pages.freedesktop.org/pipewire-rs/pipewire/)
- [XDG Desktop Portal ScreenCast API](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)

### Speech, VAD, and diarization

- [OpenAI Whisper repository/model documentation](https://github.com/openai/whisper)
- [`whisper.cpp` repository and backend/model documentation](https://github.com/ggml-org/whisper.cpp)
- [`whisper.cpp` streaming example](https://github.com/ggml-org/whisper.cpp/tree/master/examples/stream)
- [Silero VAD repository and license](https://github.com/snakers4/silero-vad)
- [`faster-whisper` repository](https://github.com/SYSTRAN/faster-whisper)
- [`sherpa-onnx` repository](https://github.com/k2-fsa/sherpa-onnx)
- [`sherpa-onnx` speaker diarization docs](https://k2-fsa.github.io/sherpa/onnx/speaker-diarization/index.html)
- [`pyannote.audio` repository](https://github.com/pyannote/pyannote-audio)
- [`pyannote community-1` model card and license](https://huggingface.co/pyannote/speaker-diarization-community-1)

### Translation, vision, and OCR

- [CTranslate2 repository and supported models/backends](https://github.com/OpenNMT/CTranslate2)
- [Ollama vision API](https://docs.ollama.com/capabilities/vision)
- [Ollama OpenAI compatibility](https://docs.ollama.com/api/openai-compatibility)
- [Ollama hardware support](https://docs.ollama.com/gpu)
- [Ollama resource/concurrency FAQ](https://docs.ollama.com/faq)
- [Ollama server license](https://github.com/ollama/ollama/blob/main/LICENSE)
- [Qwen3-VL 4B Instruct model card/license](https://huggingface.co/Qwen/Qwen3-VL-4B-Instruct)
- [Ollama Qwen3-VL artifacts](https://ollama.com/library/qwen3-vl/tags)
- [Tesseract repository, capabilities, and license](https://github.com/tesseract-ocr/tesseract)
- [Tesseract installation/language packs](https://tesseract-ocr.github.io/tessdoc/Installation.html)
- [PaddleOCR repository and license](https://github.com/PaddlePaddle/PaddleOCR)
