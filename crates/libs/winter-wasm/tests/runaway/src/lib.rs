//! An extension misbehaving: `runaway.spin` never returns, `runaway.panic`
//! panics, and `runaway.ping` says "pong" while it still runs.

use winter_ext_api::{Extension, Host, Registry};

/// What `runaway.ping` says.
const PONG: &str = "pong";

struct Runaway;

winter_ext_guest::export!(Runaway);

impl Extension for Runaway {
    fn id(&self) -> &'static str {
        "runaway"
    }

    fn activate(&self, registry: &mut Registry) {
        registry.command("runaway.ping", "Runaway: Ping", |host: &mut dyn Host| {
            host.notify(PONG)
        });
        registry.command("runaway.spin", "Runaway: Spin", |_: &mut dyn Host| {
            let mut turns: u64 = 0;
            loop {
                turns = std::hint::black_box(turns.wrapping_add(1));
            }
        });
        registry.command("runaway.panic", "Runaway: Panic", |_: &mut dyn Host| {
            panic!("on purpose")
        });
    }
}
