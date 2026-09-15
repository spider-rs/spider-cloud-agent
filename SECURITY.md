# Security policy

## Reporting a vulnerability

If you find a security problem in this crate, report it privately rather than opening
an issue.

Email: [security@spider.cloud](mailto:security@spider.cloud)

Include what you found, how to reproduce it, which versions are affected, and what an
attacker could do with it. We acknowledge reports within 48 hours and aim to ship a fix
for critical issues within 7 days.

## Scope

This policy covers the `spider-cloud-agent` and `spider-route` crates in this repository.

## Credential handling

This crate holds a Spider Cloud API key. Three things are worth knowing if you are
reviewing it for that reason.

The key is read from, in order, an explicit argument, `SPIDER_API_KEY`,
`SPIDER_CLOUD_API_KEY`, the operating system keyring, and finally `~/.spider/credentials`.
When the crate writes a key it tries the keyring first and falls back to that file at
permissions 0600. It warns once if it reads a credentials file that is readable by anyone
else, and it does not change the permissions unless you ask it to.
The keyring and the browser sign in described below are the `keyring` and `oauth`
features, both off by default, so a build that only reads a key from the environment
carries neither.

The key is never logged. Error types redact it, including in `Debug`. If you find a path
where it can reach a log line or an error message, that is a vulnerability under this
policy and we want to hear about it.

The browser login flow uses OAuth 2.1 with PKCE against a loopback redirect. It binds to
127.0.0.1 on a port the operating system assigns, checks the `state` parameter, and times
out rather than listening forever.

## Supported versions

Security fixes go to the latest release only. Use the most recent version.
