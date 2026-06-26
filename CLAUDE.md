# Winter Terminal: agent instructions

## When to run checks

Decide by **risk**, not by how big the change looks. A one-line change can break the build. A 300-line pass over comments cannot.

An "edit" is one whole change. It can span several tool calls, or a few rounds of back-and-forth fixing. Judge it when you reach a stopping point, not after every message.

| Kind of edit | What it means | What to run |
|---|---|---|
| **Cosmetic** | A comment, formatting, a constant's value, or a self-contained expression. Touches no signature and no name used anywhere else. | Nothing. Nothing else can break. |
| **Structural** | Renamed, moved, or deleted something used elsewhere. Changed a function signature or public API. Added a dependency. | `cargo check`, right away, even in the middle of the work. Do not run tests for this alone. |
| **Behavioral** | Changed logic or control flow, or anything a test could plausibly catch. | `cargo test` for the crate you touched. |

## When to run the whole suite anyway

Run `cargo test` across the workspace whenever any of these is true:

- The user asks. ("verify", "run tests", "make sure this works")
- You are about to say the task is done, and anything since your last check was Structural or Behavioral.
- Before a commit, PR, push, or merge.
- The user signals the work is over: says "looks good" or "ship it", moves to another topic, or simply stops sending tweaks.
- You are picking the work back up after a gap: a new session, a context compaction, or a switch back from something unrelated.
- You suspect something broke, no matter how small the edit was.

**The trap this rule exists to block:** finishing an edit is never, on its own, a reason to test. "I finished a complete, self-contained change, so I should verify it" is exactly the reasoning that ends in running the suite after every trivial tweak.
