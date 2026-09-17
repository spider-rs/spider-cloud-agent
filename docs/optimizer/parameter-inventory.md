# Parameter inventory

`RequestParams` now has 80 fields. The earlier inventory had 73. The fields are
declared in `spider-cloud-agent/src/params/mod.rs`, and `Schema::v1` covers all 80.
The tables use the actual Rust field types and the schema's group, learnable and
content-changing flags. `Option::None` omits a key from the wire. Even
`Some(false)` is a caller pin. "Content changing" is the schema flag.
It does not guarantee that an unflagged field cannot affect a page.
`every_request_params_field_has_a_spec` checks this inventory boundary in code.

## Transport

| Key | Type | Learnable | Content changing |
| --- | --- | --- | --- |
| `url` | `Option<String>` | No | No |
| `request` | `Option<RequestMode>` | Yes | No |
| `stealth` | `Option<bool>` | No | No |
| `fingerprint` | `Option<bool>` | No | No |
| `user_agent` | `Option<String>` | No | No |
| `viewport` | `Option<Viewport>` | No | No |
| `locale` | `Option<String>` | No | No |
| `cookies` | `Option<String>` | No | No |
| `headers` | `Option<HeaderMap>` | No | No |
| `encoding` | `Option<String>` | No | No |
| `storageless` | `Option<bool>` | No | No |
| `session` | `Option<bool>` | No | No |
| `redirect_policy` | `Option<RedirectPolicy>` | No | No |
| `request_timeout` | `Option<u8>` | No | No |
| `service_worker_enabled` | `Option<bool>` | No | No |
| `preserve_host` | `Option<bool>` | No | No |
| `delay` | `Option<u64>` | No | No |
| `concurrency_limit` | `Option<u32>` | No | No |
| `wayback` | `Option<bool>` | No | No |

## Proxy

| Key | Type | Learnable | Content changing |
| --- | --- | --- | --- |
| `proxy` | `Option<ProxyPool>` | Yes | No |
| `country_code` | `Option<Country>` | No | No |
| `remote_proxy` | `Option<String>` | No | No |

## Scope

| Key | Type | Learnable | Content changing |
| --- | --- | --- | --- |
| `limit` | `Option<u32>` | No | No |
| `depth` | `Option<u32>` | No | No |
| `budget` | `Option<CrawlBudget>` | No | No |
| `blacklist` | `Option<Vec<String>>` | No | No |
| `whitelist` | `Option<Vec<String>>` | No | No |
| `subdomains` | `Option<bool>` | No | No |
| `tld` | `Option<bool>` | No | No |
| `external_domains` | `Option<Vec<String>>` | No | No |
| `sitemap` | `Option<bool>` | No | No |
| `sitemap_only` | `Option<bool>` | No | No |
| `sitemap_path` | `Option<String>` | No | No |
| `respect_robots` | `Option<bool>` | No | No |
| `link_rewrite` | `Option<LinkRewriteRule>` | No | No |
| `crawl_timeout` | `Option<Timeout>` | No | No |

## Delivery

| Key | Type | Learnable | Content changing |
| --- | --- | --- | --- |
| `run_in_background` | `Option<bool>` | No | No |
| `webhooks` | `Option<WebhookSettings>` | No | No |
| `data_connectors` | `Option<serde_json::Map<String, serde_json::Value>>` | No | No |

## Wait

| Key | Type | Learnable | Content changing |
| --- | --- | --- | --- |
| `wait_for` | `Option<WaitFor>` | Yes | No |
| `scroll` | `Option<u32>` | No | No |

## Scripting

| Key | Type | Learnable | Content changing |
| --- | --- | --- | --- |
| `execution_scripts` | `Option<ExecutionScriptsMap>` | No | No |
| `automation_scripts` | `Option<WebAutomationMap>` | No | No |
| `evaluate_on_new_document` | `Option<String>` | No | No |

## Resources

| Key | Type | Learnable | Content changing |
| --- | --- | --- | --- |
| `disable_intercept` | `Option<bool>` | Yes | No |
| `full_resources` | `Option<bool>` | Yes | No |
| `block_ads` | `Option<bool>` | Yes | No |
| `block_analytics` | `Option<bool>` | Yes | No |
| `block_stylesheets` | `Option<bool>` | Yes | Yes |
| `disable_first_party_stylesheets` | `Option<bool>` | No | Yes |
| `disable_first_party_javascript` | `Option<bool>` | No | Yes |
| `disable_first_party_visuals` | `Option<bool>` | No | Yes |
| `network_whitelist` | `Option<Vec<String>>` | No | Yes |
| `network_blacklist` | `Option<Vec<String>>` | Yes | Yes |

## Other

| Key | Type | Learnable | Content changing |
| --- | --- | --- | --- |
| `event_tracker` | `Option<EventTracker>` | No | No |
| `router` | `Option<Router>` | No | No |
| `provider_options` | `Option<BTreeMap<String, serde_json::Value>>` | No | No |
| `disable_hints` | `Option<bool>` | No | No |
| `skip_config_checks` | `Option<bool>` | No | No |

## Extraction

| Key | Type | Learnable | Content changing |
| --- | --- | --- | --- |
| `return_format` | `Option<ReturnFormatHandling>` | No | Yes |
| `root_selector` | `Option<String>` | No | Yes |
| `exclude_selector` | `Option<String>` | No | Yes |
| `filter_output_images` | `Option<bool>` | No | Yes |
| `filter_output_svg` | `Option<bool>` | No | Yes |
| `filter_output_main_only` | `Option<bool>` | No | Yes |
| `max_size` | `Option<u64>` | No | Yes |
| `readability` | `Option<bool>` | No | Yes |
| `clean_html` | `Option<bool>` | No | Yes |
| `css_extraction_map` | `Option<CssExtractionMap>` | No | Yes |
| `chunking_alg` | `Option<ChunkingAlg>` | No | Yes |
| `metadata` | `Option<bool>` | No | No |
| `return_embeddings` | `Option<bool>` | No | No |
| `return_headers` | `Option<bool>` | No | No |
| `return_cookies` | `Option<bool>` | No | No |
| `return_page_links` | `Option<bool>` | No | No |
| `return_json_data` | `Option<bool>` | No | No |
| `text` | `Option<String>` | No | Yes |

## Cache

| Key | Type | Learnable | Content changing |
| --- | --- | --- | --- |
| `cache` | `Option<Cache>` | No | No |

## Budget

| Key | Type | Learnable | Content changing |
| --- | --- | --- | --- |
| `max_credits_allowed` | `Option<WholeCredits>` | No | No |
| `max_credits_per_page` | `Option<Credits>` | No | No |

## The five list fields

| Key | What the list controls | Optimizer treatment |
| --- | --- | --- |
| `blacklist` | Crawl paths to skip | Never edited |
| `whitelist` | Crawl paths to visit | Never edited |
| `external_domains` | Other domains a crawl can follow | Never edited |
| `network_blacklist` | Resource hosts or substrings to block | V1 validates append only, from observed candidates during generation |
| `network_whitelist` | Resource hosts or substrings to allow; wins over the network blacklist | Never edited; a caller value also pins blacklist edits |

## API gaps

The network list fields are `network_blacklist` and `network_whitelist`.
`block_images` exists only on
`ScreenshotParams`, not on `RequestParams`. The only learnable part of `wait_for`
is its idle-network timeout. A caller's selector wait pins the whole field.

`block_ads`, `block_analytics` and `block_stylesheets` default to true at the
service. `None` and `Some(true)` therefore send the same effective request. They
differ for pinning, because the optimizer cannot change an explicit value.

A blacklist edit requires `disable_hints` to be set already, and the optimizer
never sets it. Hold the same `disable_hints` setting in both arms of a comparison.

No ladder rung edits a list. The client has no merge path for a caller-supplied
list. A caller value in either network list blocks optimizer blacklist edits. On
an unpinned current request, the append keeps existing entries and removes
duplicates. That does not override the caller guard.

## Telemetry gaps

The original routing recorder rows carry no credits, attempt count, parameters
sent, later attempts, field correctness or observed traffic. Their structure has
no place for a host or body. Their `bytes` is wire bytes, not the browser's
resource transfer total. The `event_tracker` output, made of `request_map` and
`response_map`, is the only source of concrete resource identifiers. It is off
unless requested, and the optimizer never turns it on for the caller.
`SiteMemory` cannot hold a list. The client store packs its four scalar fields
into one `u64`.

| Gap | What the comparison row now covers | What remains |
| --- | --- | --- |
| Credits and attempts | Sums finite positive attempt costs and records the attempt count for a settled walk | Missing costs cannot be reconstructed, and neither can walks cut short by wall, budget or connection failure |
| Later attempts | Settled success/status, total attempt elapsed time, total costs and count | No per-attempt sequence or parameters for later ladder rungs |
| Parameters sent | Router decision, initial effective learnable configuration in features, edit descriptor and pin masks | No full serialized request or exact list entries. In shadow the descriptor was never executed |
| Field correctness | Requested/present counts and nullable pair-relative label slots | The client leaves comparison labels null. A collector must compare the values actually returned |
| Observed traffic | Optional resource buckets in edit features and identifier descriptors | A `ResourceSource` must supply observations. Rows keep no raw traffic or identifiers |
| Host and body | An opaque salted site key supports grouping | Host and body remain excluded by design, including from model inputs |

The client writes `pair: 0` and does not produce matched trials by itself.
[Dataset format](dataset-format.md) covers the collector's responsibilities.
[Rollout](rollout.md) covers the missing collector and the spend assumptions.
