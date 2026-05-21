# RPC Socket Transport

The daemon exposes a JSON-RPC 2.0 interface over a Unix domain socket. This is the primary transport for local clients like the TUI. The HTTP/WS gateway remains for webhooks, the web dashboard, and remote REST consumers.

## Socket path

The socket is created at `<data_dir>/daemon.sock`. Each `--data-dir` gets its own socket, so multiple daemon instances on the same machine do not collide.

Override with the `ZEROCLAW_SOCKET` environment variable:

```bash
export ZEROCLAW_SOCKET=/tmp/my-zeroclaw.sock
zeroclaw daemon
```

Default paths (when `ZEROCLAW_SOCKET` is not set):

| OS | Default `data_dir` | Socket path |
|---|---|---|
| Linux | `~/.local/share/zeroclaw/` | `~/.local/share/zeroclaw/daemon.sock` |
| macOS | `~/Library/Application Support/zeroclaw/` | `~/Library/Application Support/zeroclaw/daemon.sock` |

## Wire protocol

NDJSON (newline-delimited JSON). Each line is a complete JSON-RPC 2.0 message. No HTTP framing, no length prefix.

```
{"jsonrpc":"2.0","method":"initialize","params":{"protocolVersion":1,"token":"zc_..."},"id":1}\n
{"jsonrpc":"2.0","result":{"protocolVersion":1,"serverVersion":"0.8.1"},"id":1}\n
```

## Authentication

The first RPC call must be `initialize`. When `require_pairing = true` (the default), the client must send a valid pairing token. The daemon rejects all other methods until `initialize` succeeds. Protocol version mismatch produces a structured error with code `-32002`.

```json
{
  "jsonrpc": "2.0",
  "method": "initialize",
  "params": {
    "protocolVersion": 1,
    "token": "zc_abc123"
  },
  "id": 1
}
```

### Getting a pairing token

The pairing code is printed once at first daemon startup. After that:

```bash
# Check your config for existing tokens
grep paired_tokens ~/.zeroclaw/config.toml

# Generate a new one-time pairing code (while daemon is running)
zeroclaw gateway get-paircode --new
```

If `require_pairing = false` in your config, the token field is ignored and you can pass an empty string or omit it.

## Methods

| Method | Direction | Description |
|---|---|---|
| `initialize` | client -> daemon | Authenticate and negotiate protocol version |
| `session/new` | client -> daemon | Create an agent session (requires `agentAlias`, optional `cwd`, `sessionId`) |
| `session/close` | client -> daemon | Close and clean up a session |
| `session/prompt` | client -> daemon | Run a turn (streamed via `session/update` notifications) |
| `session/cancel` | client -> daemon | Cancel an in-flight turn |
| `status` | client -> daemon | Server version, protocol version, active session list |
| `session/update` | daemon -> client | Streaming notification during a turn (text chunks, tool calls, approvals) |

### Turn streaming

`session/prompt` returns the final result when the turn completes. During execution, the daemon sends `session/update` notifications with incremental events:

```json
{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"...","type":"agent_message_chunk","text":"Hello"}}
{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"...","type":"tool_call","toolCallId":"tc_1","name":"bash","rawInput":{...}}}
{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"...","type":"tool_result","toolCallId":"tc_1","name":"bash","rawOutput":"..."}}
```

Event types: `agent_message_chunk`, `agent_thought_chunk`, `tool_call`, `tool_result`, `approval_request`.

## Security

- Socket directory: `0o700` (owner only)
- Socket file: `0o600` (owner only)
- `SO_PEERCRED` on Linux provides the connecting process PID and UID for audit logging
- Pairing token validated on `initialize` when `require_pairing = true`

## Quick test

Start the daemon in one terminal:

```bash
zeroclaw daemon
```

Note the pairing code in the startup output (if pairing is enabled). In a second terminal, connect with `socat`:

```bash
socat READLINE UNIX-CONNECT:~/.local/share/zeroclaw/daemon.sock
```

Paste lines one at a time (replace the token with yours, or use empty string if pairing is off):

```
{"jsonrpc":"2.0","method":"initialize","params":{"protocolVersion":1,"token":"zc_YOURCODE"},"id":1}
{"jsonrpc":"2.0","method":"status","params":{},"id":2}
```

## Windows

Native Windows has no Unix domain socket support. Two options:

1. **WSL2 (recommended):** Run the daemon inside WSL2. Unix sockets work natively. The TUI connects over the socket from the same WSL2 instance. This is the supported path for local socket transport on Windows.

2. **Gateway only:** Use the HTTP/WS gateway (`zeroclaw daemon` on native Windows already starts the gateway). The TUI can connect over WebSocket instead of a socket. This path does not require WSL.

There is no named-pipe transport for native Windows at this time.

## Internals

The dispatch layer lives in `crates/zeroclaw-runtime/src/rpc/`:

| File | Role |
|---|---|
| `transport.rs` | `RpcTransport` trait |
| `turn.rs` | `execute_turn()` shared turn executor |
| `session.rs` | `RpcSession`, `SessionStore` |
| `dispatch.rs` | `RpcDispatcher` method routing |
| `unix.rs` | `UnixSocketTransport` + listener |

The `RpcTransport` trait is designed so that future transports (WebSocket, vsock) slot in without touching the dispatch or session logic.
