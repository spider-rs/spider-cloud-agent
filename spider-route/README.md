# spider-route

Picks the cheapest [Spider Cloud](https://spider.cloud) request settings that are likely
to work, locally and in microseconds.

The crate does no network work and runs no async runtime. It answers from rules, and
anything that implements `Router` can replace them.

See the [repository README](https://github.com/spider-rs/spider-cloud-agent) for the full
description.
