# spider-cloud-agent

An agentic client for the [Spider Cloud](https://spider.cloud) API.

It picks request settings before spending a call, reads both status planes and
escalates on its own when a fetch is blocked, and returns only what you asked for.

A provider fallback stored with `spider-agent router set` reaches the library when
you ask for it:

```rust
use spider_cloud_agent::Spider;

let spider = Spider::builder().stored_router(true).build()?;
```

Every page operation then carries the stored `router` and `provider_options`,
unless it set its own. The operation's own `router` always wins.

See the [repository README](https://github.com/spider-rs/spider-cloud-agent) for the full
description.
