# spider-optimize

Edits a [Spider Cloud](https://spider.cloud) request after `spider-route` has picked its
settings, and only when a scorer has the evidence for it.

It lists valid candidate edits to the request parameters, scores each for success,
latency and credits, and applies at most one edit set that passes validation and the
gate. Otherwise the request goes out unchanged. A field the caller set is never edited.

The crate does no network work, holds no clock and allocates nothing while it scores.
With no model compiled in, `NoModel` abstains and every request is kept.

See the [repository README](https://github.com/spider-rs/spider-cloud-agent) for the full
description.
