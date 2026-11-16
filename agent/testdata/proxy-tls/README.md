Test-only certificates for the https:// proxy tests in `agent/src/relay.rs`.

`ca.pem` is a throwaway CA (its key was deleted after signing), `proxy.pem` and
`proxy.key` are a certificate for `localhost` and `127.0.0.1` signed by it,
valid until 2126. No real host trusts this CA; the key is public on purpose.
