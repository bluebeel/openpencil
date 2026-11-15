# MCP agent contexts

Pass an optional `agentId` string in a tool call's `arguments` to give an agent
its own selection, viewport, active page, active tool and clipboard. Calls without `agentId`
retain the shared live UI behavior.

```json
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"set_selection","arguments":{"agentId":"agent-A","node_id":"n7"}}}
```

Use the same ID for subsequent `copy_selected`, `paste_clipboard`, viewport and
page calls. Selection, viewport, active page and active tool are remembered per document.
The agent's clipboard follows it across `filePath` documents. IDs accept 1–96
ASCII letters, digits, dots, underscores, colons and hyphens. Discovery advertises
the optional argument for both full and lean catalogs.

The host retains one canonical editor/document/history per resident file. Agent
contexts contain small UI records and copied clipboard nodes; they do not clone
the editor or create processes. The live user's UI context is restored after the
call, including errors and panics. Document writes persist. Undo/redo is still
shared document history, so an agent may undo another agent's edit.

UI-only opt-in calls do not rewrite the `.op` file. Content edits that trigger
persistence, and explicit saves, currently capture the calling agent's active
page in the file's editor metadata. The live user's active page still restores
in memory. This metadata policy does not change design content or geometry.

A context store belongs to one daemon/account. `agentId` identifies UI state; it
is not a credential or an authorization boundary. Existing authentication,
capability, collaboration and filesystem gates remain in force. Clients must
use distinct IDs for independent agents. One ID permits one in-flight call
across all documents; overlapping calls receive `agent-context-busy` before
waiting for a document writer lock.

The default store admits 4096 agents, 32 documents per agent and 64 MiB of
conservatively accounted context data. A clipboard is limited to 4 MiB and a
selection to 1024 entries plus its byte allowance. Capacity exhaustion returns
an explicit error; contexts are not silently evicted or assigned to another
agent. Copy/cut size is checked before applying the command. An unexpected
post-call retention limit preserves the original tool acknowledgement and adds
`result._meta["openpencil/agentContextWarning"]`; document writes are not rolled
back. A tool panic also leaves previously applied document writes intact, so
clients should inspect shared state before deciding what to do next.

Run the public synthetic benchmarks against a built headless service:

```sh
python3 scripts/bench/headless-agents.py --binary target/debug/op-host-web-server \
  --clients 10 100 1000 --max-inflight 32 --agent-contexts --output /tmp/agents-contexts
python3 scripts/bench/operation-matrix.py --binary target/debug/op-host-web-server \
  --nodes 10000 --iterations 3 --agent-contexts --output /tmp/operations-contexts
```

The first command launches one shared service, uses one distinct context per
logical client, checks every edit/ID, renders and verifies persisted content.
The second checks the deterministic A-select/B-select/A-copy-paste and
A-zoom/B-zoom/A-read interleavings, plus durable cross-file clipboard paste.
Logical clients are distinct from the bounded active request window; no external
LLM/model process is included in the service memory/CPU figures.
