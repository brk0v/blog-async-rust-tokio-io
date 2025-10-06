# Async Rust and Tokio I/O Streams: Backpressure, Concurrency, and Ergonomics

https://biriukov.dev/docs/async-rust-tokio-io

## Blog post examples

This repository backs a blog post that explores different ergonomics and performance trade-offs when driving Tokio `TcpStream`s by hand. Each binary in `src/bin` demonstrates a distinct technique, from naïve baseline code to custom high-throughput drivers.

### Client and server variants

- `src/bin/1_client_with_bug.rs` – Single task that uses `tokio::select!` to juggle reads and writes on the same `TcpStream`. Intentionally buggy: backpressure on the socket quickly stalls the loop.
- `src/bin/1_client_with_bug_cancel_select.rs` – Adds cooperative cancellation with `CancellationToken`, splits the stream, and wires Ctrl+C handling to abort in-flight IO safely.
- `src/bin/1_server_recvbuf_set.rs` – Companion server that tweaks `SO_RCVBUF` via `socket2` to exaggerate client-side backpressure behaviour.
- `src/bin/2_client_split.rs` – Minimal working example that `split`s the stream and runs dedicated reader / writer tasks without extra abstractions.
- `src/bin/3_client_split_generics.rs` – Generalises the split pattern over any `AsyncRead + AsyncWrite` transport so the logic can be reused in tests or with TLS wrappers.
- `src/bin/4_client_driver.rs` – Uses the bespoke `Connection` driver which multiplexes an mpsc channel onto a buffered stream for higher write throughput and read fan-out.
- `src/bin/5_client_driver_framed.rs` – Demonstrates the framed variant built on `ConnectionFramed` with `LengthDelimitedCodec` for length-prefixed messaging.
- `src/bin/5_server_framed.rs` – Framed echo/heartbeat server that pairs with the framed client and showcases `FramedRead`/`FramedWrite` usage.

### Running the examples

Pick a client (and the matching server when required) and run with cargo:

```
cargo run --bin 1_server_recvbuf_set
cargo run --bin 2_client_split
```

Some examples assume a server is already listening on `127.0.0.1:8080`. Use the framed server when exercising the framed clients.

Happy hacking!
