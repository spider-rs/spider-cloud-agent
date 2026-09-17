# spider-cloud-agent

An agent for other agents, on the [Spider Cloud](https://spider.cloud) API.

It picks request settings before spending a call, reads both status planes and
escalates on its own when a fetch is blocked, and returns only what you asked for.
Every reply is a typed record for a program to read. Deciding what to send costs
no language model call, so an agent that embeds this pays tokens for the page and
nothing for the plumbing.

The library uses a provider fallback stored with `spider-agent router set` only
when you ask for it:

```rust
use spider_cloud_agent::Spider;

let spider = Spider::builder().stored_router(true).build()?;
```

Every page operation then carries the stored `router` and `provider_options`,
unless it sets its own. The operation's own `router` always wins.

See the [repository README](https://github.com/spider-rs/spider-cloud-agent) for the full
description.
