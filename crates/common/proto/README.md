The source is XTLS/Xray-core tag v26.10.31, app/stats/command/command.proto.
Upstream: https://github.com/XTLS/Xray-core/tree/v26.10.31/app/stats/command
The MPL-2.0 license is included as LICENSE.xray. stats.rs implements only the
QueryStats request/response field tags and StatsService/QueryStats RPC path
using prost and tonic. It does not expose HandlerService or counter reset.
Runtime decoding is bounded to 64 KiB and 512 counters (256 stable clients).
No protoc, Go compiler or Node is required in production.

Pinned source SHA-256:
`aadeb6b30e2bd13782896fa7addba101f522ea99438f9511aa4d8f1d62dcf58a`.
Git blob: `c2f7405414ec81eb0fd7804fb52fec9bb71c6efa`.
