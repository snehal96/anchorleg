# Permission approval through relay (claude 2.1.294, 2026-10-08)

- `probe-tool-check.jsonl`: a bare probe mod's `tool.check` hook in `claude -p` (default
  permission mode). Core's verdict for a Write was `ask` ("…you haven't granted it yet"); the
  hook answered `allow` and the file was written, `permission_denials: []`. 2 Haiku requests.
- `run-write-approved.jsonl`: `relay run --model haiku` with relay-mod. Claude asked to Write
  hello.txt; relay-mod called `relay permission ask`; the answer `relay permission answer 1 yes`
  let it through (file written, result "done"). 2 Haiku requests.
