# winter-proto

Terminal Block Protocol (TBP) v1: wire types and the reference codec.

TBP is an OSC escape carrying a MIME bundle, directly inspired by Jupyter's `display_data` message. A tool emits a block as one escape; the terminal selects the richest representation it can render and falls back toward `text/plain`. Terminals that do not implement TBP ignore the escape wholesale, so emitting one is safe anywhere.

A message is `OSC 9001 ; <verb> ; <params> ; <base64 payload> ST`, where the verbs are `emit`, `open`, `patch`, `close`, and `caps`.

```rust
use serde_json::json;
use winter_proto::{encode, BlockId, EmitBlock, Message, MimeBundle, TrustTier};

let mut bundle = MimeBundle::new();
bundle.insert("text/markdown", json!("# hello"));
bundle.insert("text/plain", json!("hello"));

let escape = encode(&Message::Emit(EmitBlock {
    bundle,
    id: BlockId(1),
    trust: TrustTier::Restricted,
}));
print!("{escape}");
```

## Trust tiers

A tier on the wire is a request, never a grant. Every byte reaching a terminal is attacker-controlled, so `TrustTier::clamp_to` lowers a requested tier to the policy ceiling and never raises it. Tiers are ordered by capability (`Isolated < Restricted < Trusted`) and the clamp relies on that ordering.

```rust
use winter_proto::TrustTier;

assert_eq!(
    TrustTier::Trusted.clamp_to(TrustTier::Restricted),
    TrustTier::Restricted
);
```

## License

MIT. See [LICENSE](LICENSE).
