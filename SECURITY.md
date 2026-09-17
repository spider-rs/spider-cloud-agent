# Security policy

## Reporting a vulnerability

If you find a security problem in this crate, report it privately rather than opening
an issue.

Email: [security@spider.cloud](mailto:security@spider.cloud)

Include what you found, how to reproduce it, which versions are affected, and what an
attacker could do with it. We acknowledge reports within 48 hours and aim to ship a fix
for critical issues within 7 days.

## Scope

This policy covers `spider-cloud-agent`, `spider-route` and `spider-agent-cli`.
The CLI runs `login`, binds the OAuth loopback listener and stores the key.

## Credential handling

This crate holds a Spider Cloud API key. Three things are worth knowing if you are
reviewing it for that reason.

The key is read from, in order, an explicit argument, `SPIDER_API_KEY`,
`SPIDER_CLOUD_API_KEY`, the operating system keyring when the `keyring` feature is
enabled, and finally `~/.spider/credentials`. The first non-empty value after
trimming wins. When the crate writes a key it tries the keyring when enabled and
falls back to that file. On Unix, the file is created at mode 0600. Reading a file
with broader permissions logs a warning once; `Credentials::tighten_file_permissions`
narrows its mode only when called. The permission check and tightening are Unix
only. On Windows, the file uses the default ACL; the keyring is tried first if
that feature is enabled.

The library's `keyring` and `oauth` features are both off by default.
`spider-agent-cli` enables `oauth` by default and leaves `keyring` off. Build the
CLI with `--features keyring` to enable keyring storage, or
`--no-default-features` to omit browser sign-in.

The key is never logged. Error types redact it, including in `Debug`. If you find a path
where it can reach a log line or an error message, that is a vulnerability under this
policy and we want to hear about it.

The browser login flow uses OAuth 2.1 with PKCE against a loopback redirect. It binds to
127.0.0.1 on a port the operating system assigns, checks the `state` parameter, and times
out rather than listening forever.

## Release signing and self update

`spider-agent` updates itself from the GitHub release marked latest. Each release
signs its `SHA256SUMS.txt` with minisign, key id `43EBB605B452BC13`. The public key
is pinned in `spider-agent-cli/src/update/release-keys.pub` and compiled into the
binary. A release build trusts only the keys in that file, and no environment
variable or flag adds one.

Before reading the sums file, the updater verifies the signature and requires the
signed trusted comment to be `spider-agent v<version> SHA256SUMS.txt` for the tag it
is installing. It then checks the archive against the sums file, and it installs
nothing that fails either step. The secret key stays on the release machine, outside
every repository. A way to install an update without a valid signature, or to make a
release build trust another key, is a vulnerability under this policy.

## Supported versions

Security fixes go to the latest release only. Use the most recent version.
