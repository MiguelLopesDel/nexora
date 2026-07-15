# Provider capability and cost contract research

**Research date:** 2026-07-15 UTC

**Scope:** Nexora v1 provider/model catalog for OpenAI, Anthropic, Google Gemini,
DeepSeek, and OpenAI-compatible/local runtimes represented by Ollama.

**Source policy:** official provider documentation and first-party API references
only.

> **Volatility warning:** model IDs, aliases, supported controls, prices, preview
> status, and retirement dates are all volatile. This document is a dated
> snapshot. The catalog should store source and freshness metadata and must not
> compile the values below into timeless application logic.

## Executive conclusion

The catalog needs two contracts, not a single flat model table:

1. A **capability contract** that describes what a model can do on a particular
   provider API surface, including the exact control shape and maturity.
2. A **metering and price contract** that separately records what the provider
   reports as usage and how a dated price schedule converts that usage into
   money.

Simple booleans such as `vision: true` and a pair of `input_price` /
`output_price` fields are insufficient. The official APIs distinguish image
input from image generation, file audio from realtime speech, JSON mode from
schema-constrained output, client functions from hosted tools, and a boolean
thinking switch from a real effort control. Billing can distinguish modality,
cache reads and writes, reasoning tokens, tool calls, storage time, service
tier, geography, long-context thresholds, and subscription/local compute.

Discovery is also not uniform. Anthropic currently returns unusually rich
capability metadata from `GET /v1/models`; Gemini returns limits, supported
generation methods, and a thinking flag; OpenAI and DeepSeek return mostly
identity metadata; Ollama requires both `/api/tags` and `/api/show`. No surveyed
model-list endpoint is a complete, authoritative live price feed. Therefore,
remote discovery must be merged with a dated, curated registry and optional
runtime probes, never treated as the whole catalog.

## Recommended capability contract

### Identity and lifecycle

Store provider identity separately from the model identifier sent on the wire.
At minimum:

```text
ProviderModel {
  provider_id
  api_surface                 # responses, messages, generateContent, chat, etc.
  model_id                    # exact request value
  canonical_family_id         # optional stable grouping
  display_name
  version_kind                # snapshot | stable | rolling_alias | preview | experimental
  resolved_model_id           # if the provider reports what an alias resolved to
  release_at
  lifecycle_status            # active | legacy | deprecated | retired | unknown
  deprecation_announced_at
  shutdown_not_before_at
  shutdown_at
  replacement_model_id
  provider_metadata_raw
  discovered_at
}
```

Do not infer lifecycle solely from an ID suffix. Gemini formally distinguishes
stable, preview, latest, and experimental names, and warns that a `latest` alias
is hot-swapped; OpenAI model pages distinguish aliases and snapshots; DeepSeek
has historically repointed aliases to newer underlying models; Ollama tags may
refer to mutable local artifacts identified more precisely by digest. See the
[Gemini model naming guide](https://ai.google.dev/gemini-api/docs/models),
[OpenAI model catalog](https://developers.openai.com/api/docs/models),
[DeepSeek changelog](https://api-docs.deepseek.com/updates/), and
[Ollama list-models response](https://docs.ollama.com/api/tags).

### Capability values must be tri-state and sourced

Use `supported | unsupported | unknown`, not a boolean. An omitted field in a
minimal discovery API means unknown, not false. Each capability should carry:

```text
CapabilitySupport {
  state                        # supported | unsupported | unknown
  maturity                     # ga | beta | preview | experimental | unknown
  source_kind                  # provider_api | official_docs | runtime_probe | user_override
  source_url
  observed_at
  notes
}
```

Merge precedence should be explicit: a user override is local policy; a recent
successful runtime probe is evidence for that endpoint; provider discovery is
preferred for fields it actually defines; curated official-doc data fills
gaps. A failed request caused by account access or region must not automatically
mark a model capability unsupported globally.

### Model the directions, modes, and transport independently

Recommended capability groups:

```text
modalities.input:  { text, image, audio, video, pdf }
modalities.output: { text, image, audio, video }

generation:
  streaming_http                 # SSE or NDJSON response streaming
  streaming_transport            # sse | ndjson | websocket | none | unknown
  realtime_bidirectional
  conversation_state_server_side

tools:
  client_function_calling
  parallel_function_calling
  strict_function_arguments
  hosted_tools[]                 # web search, code execution, file search, etc.

structured_output:
  json_object                    # syntactically valid JSON only
  json_schema                    # schema constrained
  strict_schema                  # provider guarantee, not merely prompting
  schema_dialect_or_subset

reasoning:
  supported
  exposed_content                # none | summary | full_trace | model_dependent
  control_kind                   # none | boolean | effort_enum | token_budget | adaptive
  accepted_efforts[]             # exact effective values for this model/surface
  default_effort
  can_disable
  budget_min_tokens
  budget_max_tokens
  usage_counter_path

limits:
  input_context_tokens
  max_output_tokens
  max_images
  max_audio_duration
```

The contract should allow multiple reasoning controls because providers change
control shape between model generations. It should also distinguish **accepted
compatibility values** from **effective values**. DeepSeek, for example,
documents `high` and `max` as the real effort levels while mapping `low` and
`medium` to `high`, and `xhigh` to `max`; recording all accepted strings as
distinct capabilities would be misleading. Anthropic explicitly calls effort a
behavioral signal rather than a strict token budget. Gemini 3 uses relative
thinking levels while Gemini 2.5 uses a token budget. OpenAI states that valid
effort values and defaults are model-dependent. Sources:
[DeepSeek Chat Completion reference](https://api-docs.deepseek.com/api/create-chat-completion),
[Anthropic effort guide](https://platform.claude.com/docs/en/build-with-claude/effort),
[Gemini thinking guide](https://ai.google.dev/gemini-api/docs/thinking), and
[OpenAI reasoning guide](https://developers.openai.com/api/docs/guides/reasoning).

### Endpoint support is not model capability

Keep `api_surface` and endpoint support in the provider adapter. A provider can
offer separate text, image, audio, and realtime endpoints, and a model may be
valid on only some of them. OpenAI's model pages explicitly list modalities,
endpoints, features, and hosted tools per model; Gemini separates
`generateContent`, `streamGenerateContent`, Interactions, and Live API use
cases; Ollama exposes a native API plus partial OpenAI and Anthropic
compatibility. An `openai_compatible` flag therefore means protocol ancestry,
not semantic parity.

## Recommended usage and price contract

### Preserve raw usage, then normalize without double counting

Every completed response should retain the provider's raw usage object and a
normalized record:

```text
UsageRecord {
  provider_id
  model_id_requested
  model_id_reported
  api_surface
  request_id
  occurred_at
  service_tier

  input_tokens_total
  input_tokens_uncached
  cache_read_tokens
  cache_write_tokens
  input_tokens_by_modality      # text/image/audio/video/other

  output_tokens_total
  reasoning_tokens
  output_tokens_by_modality
  tool_prompt_tokens

  tool_uses_by_kind             # e.g. web_search_requests
  image_units_by_quality_size
  audio_seconds_in
  audio_seconds_out
  cache_storage_token_seconds
  runtime_seconds

  provider_total_tokens
  total_semantics               # inclusive | exclusive | unknown
  raw_usage_json
}
```

Normalization rules must be provider-specific:

- OpenAI Responses reports `input_tokens` with nested `cached_tokens`, and
  `output_tokens` with nested `reasoning_tokens`; nested values are breakdowns,
  not extra tokens to add to their parent totals. See the
  [Responses API reference](https://developers.openai.com/api/reference/resources/responses/methods/create).
- Anthropic states that total input is the sum of `input_tokens`,
  `cache_creation_input_tokens`, and `cache_read_input_tokens`. Its current
  usage schema can also report cache TTL breakdowns, thinking tokens, and
  server-tool request counts. See the
  [Messages API type reference](https://platform.claude.com/docs/en/api/typescript/messages)
  and [extended-thinking accounting](https://platform.claude.com/docs/en/build-with-claude/extended-thinking).
- Gemini `promptTokenCount` already includes cached content;
  `cachedContentTokenCount` is a subset. It separately reports candidates,
  thoughts, tool-use prompt tokens, totals, modality detail arrays, and service
  tier. See `UsageMetadata` in the
  [Generate Content reference](https://ai.google.dev/api/generate-content).
- DeepSeek reports prompt totals split into cache hit and miss, completion
  totals, and a `completion_tokens_details.reasoning_tokens` breakdown. See the
  [DeepSeek Chat Completion reference](https://api-docs.deepseek.com/api/create-chat-completion).
- Ollama reports prompt and generated token counts plus evaluation/load timing,
  but its native response does not provide the surveyed cache/reasoning billing
  breakdowns. See [Ollama usage](https://docs.ollama.com/api/usage).

Usage from interrupted or timed-out streams should be marked partial unless a
provider final usage event was received. DeepSeek requires
`stream_options.include_usage` for a final whole-request usage chunk; Anthropic
places cumulative output usage in stream deltas; Ollama puts final metrics on
the completed NDJSON object. Sources: [DeepSeek streaming schema](https://api-docs.deepseek.com/api/create-chat-completion),
[Anthropic streaming](https://platform.claude.com/docs/en/build-with-claude/streaming),
and [Ollama streaming](https://docs.ollama.com/api/streaming).

### A price is a dated rule, not a model attribute

Recommended shape:

```text
PriceSchedule {
  provider_id
  model_match                   # exact ID/family/alias, with precedence
  api_surface
  currency
  effective_from
  effective_until
  billing_basis                 # metered | subscription | local_compute | unknown
  source_url
  fetched_at
  entries[]
  modifiers[]
}

PriceEntry {
  meter                         # token | image | second | minute | request |
                                # tool_call | token_second | session_hour | gpu_time
  direction                     # input | output | storage | runtime
  modality                      # text | image | audio | video | any
  cache_class                   # miss | read | write_5m | write_1h | none
  token_class                   # regular | reasoning | tool_prompt | intermediate
  tool_kind
  quality
  dimensions                    # image size, resolution, etc.
  unit_quantity                 # e.g. 1_000_000 tokens or 1_000 calls
  unit_price_decimal
  included_in_parent_meter      # avoids charging a breakdown twice
}

PriceModifier {
  kind                          # batch | flex | priority | geography |
                                # long_context | data_sharing | temporary_offer
  predicate
  multiplier_or_override
}
```

This shape is required by current first-party pricing:

- OpenAI publishes separate text, audio, and image token rates, cached input,
  cache-write and long-context distinctions for some models, per-image
  estimates for image generation, per-minute speech products, tool-call fees,
  and processing-tier differences. [OpenAI pricing](https://developers.openai.com/api/docs/pricing).
- Anthropic distinguishes base input, 5-minute and 1-hour cache writes, cache
  hits, output, batch discounts, geography multipliers, fast mode, and
  separately metered server tools/runtime. [Anthropic pricing](https://platform.claude.com/docs/en/about-claude/pricing).
- Gemini prices text/image/video input differently from audio on some models,
  includes thinking tokens in output pricing, charges cache token use plus
  storage duration, and has standard, batch, flex, and priority tiers plus
  tool/grounding charges. [Gemini pricing](https://ai.google.dev/gemini-api/docs/pricing).
- DeepSeek currently publishes different input cache-hit, input cache-miss, and
  output rates and warns that prices may change. [DeepSeek models and pricing](https://api-docs.deepseek.com/quick_start/pricing/).
- Local Ollama has no provider token invoice; the economic cost is user-owned
  compute and should be `local_compute`, not falsely `$0`. Ollama Cloud is
  subscription/utilization based rather than a fixed per-token schedule.
  [Ollama pricing](https://ollama.com/pricing).

Use decimal arithmetic and retain the currency and unit denominator. A missing
rate is `unknown`, never zero. Estimate output should include line items and a
confidence such as `exact_from_reported_usage`, `estimated_from_tokens`,
`subscription_not_allocated`, or `unknown`.

### Reasoning and multimodal charging

Do not add a universal `reasoning_price`. Providers usually charge reasoning as
output while exposing it as a breakdown:

- OpenAI nests reasoning tokens under output usage.
- Anthropic says `output_tokens` remains the inclusive authoritative billing
  total and `output_tokens_details.thinking_tokens` is observability detail.
- Gemini states response cost includes output plus thinking tokens and exposes
  `thoughtsTokenCount`.
- DeepSeek exposes reasoning inside completion-token details; preserve the raw
  total semantics and mark the relationship unknown if a provider version does
  not state whether it is inclusive.

Likewise, `image_count` alone is not enough. Anthropic and Gemini generally
tokenize image input; OpenAI can price image-token input/output and publish
per-image generation estimates; audio may be per token or per minute. Sources:
[Anthropic vision](https://platform.claude.com/docs/en/build-with-claude/vision),
[Gemini token accounting](https://ai.google.dev/gemini-api/docs/tokens), and
[OpenAI image model pricing example](https://developers.openai.com/api/docs/models/gpt-image-1).

## Provider findings relevant to Nexora v1

The following is a contract-oriented comparison, not an exhaustive model list.
`Model-dependent` means the catalog must resolve support at model and API
surface level.

| Provider | Image | Audio | Streaming | Tools | Structured output | Thinking / real control | Discovery and pricing implication |
|---|---|---|---|---|---|---|---|
| OpenAI | Image input on current general models; separate image generation/edit models and hosted image tool | Specialized transcription, speech, audio, and Realtime models; input/output varies by model | HTTP SSE; WebSocket/Realtime on relevant surfaces | Client functions plus model-specific hosted tools | JSON Schema/strict support is model-specific | Model-specific `reasoning.effort`; documented values can include `none`, `minimal`, `low`, `medium`, `high`, `xhigh` | `/v1/models` is identity-oriented; merge official model pages, pricing, and deprecations |
| Anthropic | Image and PDF input; no audio content capability is advertised in the surveyed Messages model-capability schema | No native audio modality documented in surveyed Messages API | SSE with text, tool, and thinking deltas | Client functions and server tools; strict tool schemas on supported models | `output_config.format` JSON Schema plus `strict: true` tools on supported models | Manual token budget on older models; adaptive thinking plus model-specific effort on newer models | `/v1/models` exposes rich capabilities and limits, but pricing/lifecycle still require official schedules |
| Gemini | Broad image input; specialized Gemini/Imagen image output | File audio understanding; Live API/native-audio models for realtime audio; model-dependent output | `streamGenerateContent` SSE; Live API for bidirectional realtime | Function calling plus built-in tools | JSON Schema subset; tool combination varies by model/preview | Gemini 3 `thinking_level`; Gemini 2.5 `thinkingBudget`; defaults and disable support vary | Models API exposes limits/methods/thinking, not a complete modality/cost contract |
| DeepSeek | Current official Chat Completion user content schema is text-only | No native audio field in current official Chat Completion schema | Data-only SSE | Function tools; strict tool mode is beta; thinking-mode tool use supported in current generation | `json_object` guarantees valid JSON; do not equate it with arbitrary strict schema; strict applies to beta tools | Thinking enable/disable plus real effort levels `high` and `max`; compatibility values collapse to these | `/models` gives ID/object/owner only; merge pricing and changelog/deprecation data |
| Ollama | Vision on models whose `/api/show` capabilities include it | No native audio content capability documented in surveyed native chat API | Native API streams NDJSON by default | Model-dependent function tools | Native local API accepts JSON or schema; official docs say Ollama Cloud currently does not support structured outputs | Most thinking models use boolean `think`; GPT-OSS uses `low/medium/high` | `/api/tags` lists installed models; `/api/show` adds capabilities/model info; local cost is external compute |

### OpenAI

OpenAI's catalog demonstrates why capabilities must be per model. Official model
pages separately enumerate text/image/audio/video directions, endpoint support,
streaming, function calling, structured outputs, tools, snapshots, prices, and
deprecation markings. A general model may accept images but not audio, while a
Realtime model can accept and produce audio, and an image model can produce
images but lack function calling. See the
[OpenAI models catalog](https://developers.openai.com/api/docs/models),
[a general model capability page](https://developers.openai.com/api/docs/models/gpt-4o),
and [a Realtime model capability page](https://developers.openai.com/api/docs/models/gpt-realtime-1.5).

`GET /v1/models` currently returns `id`, `object`, `created`, and `owned_by`; it
does not carry the rich per-model capability and price grid shown on the model
pages. [List Models reference](https://developers.openai.com/api/reference/resources/models/methods/list).

Structured output and function/tool arguments should be separate flags even
when both use JSON Schema. OpenAI's guide uses strict schema configuration, and
streaming over HTTP uses SSE with typed events. Sources:
[Structured Outputs](https://developers.openai.com/api/docs/guides/structured-outputs)
and [streaming Responses](https://developers.openai.com/api/docs/guides/streaming-responses).

Lifecycle must be refreshed independently. The official deprecations page
records announcement/shutdown dates and replacements, while aliases and
snapshots appear on model pages. [OpenAI deprecations](https://developers.openai.com/api/docs/deprecations).

### Anthropic

Anthropic's current Models API is the strongest discovery source surveyed. Its
`ModelInfo` includes ID, display name, release time, input/output limits, and
capability objects for image/PDF input, structured outputs, thinking types,
effort levels, batch, citations, code execution, and context management. The
result is paginated and newest-first. [Anthropic List Models](https://platform.claude.com/docs/en/api/models/list).

This rich response still does not remove the need for a curated layer: pricing
and retirement policy live in separate pages, capabilities can depend on API
version/beta headers and hosting platform, and no audio capability is present in
the surveyed model-capability schema. Treat audio as unsupported for the native
Messages adapter until official documentation or a runtime response says
otherwise, not as a general statement about every Anthropic-adjacent platform.

Anthropic exposes two structured guarantees: schema-constrained final JSON via
`output_config.format`, and strict tool arguments via `strict: true`. Both use a
supported JSON Schema subset. [Structured outputs](https://platform.claude.com/docs/en/build-with-claude/structured-outputs)
and [strict tool use](https://platform.claude.com/docs/en/agents-and-tools/tool-use/strict-tool-use).

Lifecycle status has explicit `active`, `legacy`, `deprecated`, and `retired`
meanings, with replacement and retirement dates. Schedules can differ on
partner-operated platforms, so lifecycle is keyed by provider surface, not only
model family. [Anthropic model deprecations](https://platform.claude.com/docs/en/docs/about-claude/model-deprecations).

### Google Gemini

The Gemini Models API returns model resource name, base ID, version, display
name, description, input/output token limits, supported generation methods,
whether thinking is supported, and sampling defaults/limits. It does not expose
the complete per-model modality, tools, structured-output, or price matrix, so
the result still needs curated augmentation. [Gemini Models API](https://ai.google.dev/api/models).

Gemini supports image, audio, video, and document input across applicable
models, but realtime transcription/voice belongs to the Live API rather than
ordinary file-audio understanding. Image generation is a separate output
capability on designated Gemini/Imagen models. Sources:
[image understanding](https://ai.google.dev/gemini-api/docs/image-understanding),
[audio understanding](https://ai.google.dev/gemini-api/docs/audio), and
[image generation](https://ai.google.dev/gemini-api/docs/image-generation).

The structured-output API supports a subset of JSON Schema and is conceptually
different from function calling. Thinking controls and defaults vary materially
by generation/model. Sources:
[Gemini structured output](https://ai.google.dev/gemini-api/docs/structured-output),
[function calling](https://ai.google.dev/gemini-api/docs/function-calling), and
[thinking](https://ai.google.dev/gemini-api/docs/thinking).

The lifecycle registry should store an earliest shutdown separately from an
exact shutdown: Google's deprecation page says table dates can be the earliest
possible retirement, with exact dates communicated later. [Gemini deprecations](https://ai.google.dev/gemini-api/docs/deprecations).

### DeepSeek

DeepSeek's current OpenAI-format API is intentionally familiar but not
identical. The current Chat Completion reference documents text messages,
thinking on/off, `reasoning_effort`, JSON-object output, SSE, function tools,
cache usage, and reasoning-token detail. The model-list response contains only
`id`, `object`, and `owned_by`, so capability discovery by `/models` alone is
unsafe. Sources: [Chat Completion](https://api-docs.deepseek.com/api/create-chat-completion)
and [List Models](https://api-docs.deepseek.com/api/list-models).

The current pricing page (dated by this research, not copied as a permanent
constant) advertises V4 Flash/Pro, both thinking and non-thinking modes, JSON,
tool calling, cache-hit/miss pricing, and a scheduled retirement for legacy
aliases. It explicitly reserves the right to change prices. [DeepSeek models
and pricing](https://api-docs.deepseek.com/quick_start/pricing/).

DeepSeek's beta strict mode guarantees tool-call arguments against a supported
schema subset, but JSON Output itself is `json_object`, not a final-response JSON
Schema contract. [DeepSeek tool calls](https://api-docs.deepseek.com/guides/tool_calls).

### Ollama and arbitrary OpenAI-compatible endpoints

Ollama's native `/api/tags` describes installed artifacts (name, timestamp,
size, digest, family, parameters, quantization). `/api/show` adds a capability
array and raw model metadata, so the adapter should call `show` lazily for
selected models rather than assume the tag name reveals vision/tools/thinking.
[List models](https://docs.ollama.com/api/tags) and
[show model details](https://docs.ollama.com/api-reference/show-model-details).

Native chat supports model-dependent image input, tools, thinking, structured
format, and streaming. Streaming is NDJSON by default rather than the SSE used
by OpenAI Chat Completions. Thinking is usually boolean, but GPT-OSS uses an
effort-like enum. Sources: [native chat](https://docs.ollama.com/api/chat),
[streaming](https://docs.ollama.com/api/streaming),
[tool calling](https://docs.ollama.com/capabilities/tool-calling), and
[thinking](https://docs.ollama.com/capabilities/thinking).

Ollama is also a warning against treating a provider-level capability as
universal: its official structured-output guide says the native local feature
accepts JSON/JSON Schema but Ollama Cloud currently does not support structured
outputs. [Ollama structured outputs](https://docs.ollama.com/capabilities/structured-outputs).

For an arbitrary OpenAI-compatible base URL, default all optional capabilities
to `unknown`, obtain the model list if available, allow a user-selected adapter
profile, and optionally run non-destructive probes. Do not infer images, audio,
tools, strict schema, reasoning effort, usage breakdown, or prices merely from
`/v1/models` succeeding. Preserve unrecognized response fields in raw metadata
so the adapter can improve without losing evidence.

## Refresh and validation policy

Recommended v1 operational rules:

1. Ship a signed/curated baseline with `fetched_at`, `source_url`, and explicit
   `unknown` values.
2. Refresh model discovery on user request or a conservative TTL; do not poll in
   the background.
3. Refresh volatile price and lifecycle documents more frequently than stable
   protocol facts, but never silently rewrite historical cost calculations.
4. Attach the exact `PriceSchedule` version to each calculated cost.
5. Keep model aliases and snapshots distinct. If a response returns a resolved
   model ID, persist both requested and reported IDs.
6. Warn before use when `shutdown_at` is near; block only when the provider has
   retired the model or the request actually fails, unless user policy is
   stricter.
7. Treat preview/beta features and provider-specific headers/base URLs as part
   of the capability record.
8. Unit-test normalization with provider fixture payloads, especially cached
   token subset/sum semantics and stream-final usage.

## Suggested v1 acceptance fixtures

A small fixture set can prove the contract without live API calls:

- OpenAI Responses usage with cached input and reasoning output; one general
  vision model and one audio/realtime model capability record.
- Anthropic Messages usage with base input, 5-minute/1-hour cache writes, cache
  reads, thinking detail, and server-tool count; one discovered `ModelInfo` with
  adaptive thinking and explicit effort levels.
- Gemini usage with cached content, thought tokens, modality arrays, tool-use
  tokens, and service tier; one Gemini 3 thinking-level record and one Gemini
  2.5 budget record.
- DeepSeek usage with cache hit/miss and reasoning detail; verify that accepted
  `low` normalizes to effective `high` only when the official profile says so.
- Ollama final NDJSON response with `prompt_eval_count`, `eval_count`, and
  durations; `/api/tags` plus `/api/show` merge; local-compute cost remains
  unknown rather than zero.

These fixtures should assert both normalized fields and the untouched raw JSON.
They should also verify that absent price metadata produces an unknown estimate,
not a zero-cost estimate.
