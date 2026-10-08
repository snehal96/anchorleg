# Codex app-server quota read (codex-cli 0.153.4, bundled in ChatGPT.app, 2026-10-08)

`codex app-server` (stdio JSON-RPC) with `initialize` (`clientInfo`), the `initialized`
notification, then `{"id":2,"method":"account/rateLimits/read"}`. No model request; about 2-3 s.

- Line 1: the answer for a logged-in free-plan account (`accountId` zeroed).
- Line 2: the answer with an empty `CODEX_HOME` (not logged in).

Schema: `codex app-server generate-json-schema --out DIR` → `v2/GetAccountRateLimitsResponse.json`.
