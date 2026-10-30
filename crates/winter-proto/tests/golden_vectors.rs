//! The shared TBP golden vectors (`docs/tbp-golden-vectors.json`), checked
//! against the reference codec and the shell client.
//!
//! The Rust, Python, and shell clients each frame escapes on their own; a
//! difference between them shows up as blocks one client emits and another
//! cannot be decoded alongside. Every client is held to the same vectors:
//! the Python ones in `clients/client-py/tests/test_vectors.py`, and the
//! Rust codec and `clients/client.sh` here.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use serde_json::Value;
use winter_proto::{
    decode, encode, BlockId, EmitBlock, Message, MimeBundle, OpenBlock, PatchBlock, TrustTier,
    BLOCK_SEQUENCE_BITS,
};

// ========================================================================
// Constants
// ========================================================================

/// The vectors, relative to this crate.
const VECTORS_PATH: &str = "../../docs/tbp-golden-vectors.json";

/// The shell client, relative to this crate.
const CLIENT_SH_PATH: &str = "../../clients/client.sh";

const OSC_START: &str = "\x1b]";
const ST: &str = "\x1b\\";

// ========================================================================
// Harness
// ========================================================================

fn crate_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// Every vector in the shared file.
fn vectors() -> Vec<Value> {
    let text = std::fs::read_to_string(crate_path(VECTORS_PATH)).expect("vectors file");
    let doc: Value = serde_json::from_str(&text).expect("vectors are JSON");
    doc["vectors"].as_array().expect("a vectors array").clone()
}

fn id_of(vector: &Value) -> BlockId {
    BlockId(vector["id"].as_u64().expect("an id"))
}

fn str_of<'a>(vector: &'a Value, key: &str) -> &'a str {
    vector[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} in {vector}"))
}

/// The message a vector describes.
fn message_of(vector: &Value) -> Message {
    match str_of(vector, "verb") {
        "close" => Message::Close(id_of(vector)),
        "emit" => Message::Emit(EmitBlock::new(id_of(vector), TrustTier::from_str(str_of(vector, "trust")).expect("a tier"), serde_json::from_value::<MimeBundle>(vector["bundle"].clone())
                .expect("a bundle"))),
        "open" => Message::Open(OpenBlock::new(id_of(vector), str_of(vector, "mime").to_string(), vector["spec"].clone())),
        "patch" => Message::Patch(PatchBlock::new(id_of(vector), vector["patch"].clone())),
        other => panic!("unknown verb {other}"),
    }
}

fn strip_frame(escape: &str) -> &str {
    escape
        .strip_prefix(OSC_START)
        .and_then(|rest| rest.strip_suffix(ST))
        .expect("an OSC-framed escape")
}

/// Run `script` in bash with `client.sh` sourced, inside Winter and outside
/// any multiplexer, returning what it printed. Its live blocks belong to
/// process 0, so a vector's id is written as it is (see
/// `test_client_sh_scopes_a_live_block_s_id_by_its_process`).
fn run_client_sh(script: &str) -> String {
    run_client_sh_as(script, "0")
}

/// The same, its live blocks belonging to process `pid`.
fn run_client_sh_as(script: &str, pid: &str) -> String {
    let source = format!(". '{}'; {script}", crate_path(CLIENT_SH_PATH).display());
    let output = Command::new("bash")
        .arg("-c")
        .arg(source)
        .env("WINTER", "1")
        .env("WINTER_BLOCK_PID", pid)
        .env_remove("TMUX")
        .env_remove("STY")
        .output()
        .expect("bash runs");
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).expect("UTF-8 output")
}

/// The single-quoted shell word for `text`.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn bash_available() -> bool {
    Command::new("bash")
        .arg("-c")
        .arg("true")
        .status()
        .is_ok_and(|status| status.success())
}

// ========================================================================
// Tests
// ========================================================================

#[test]
fn test_the_codec_writes_and_reads_every_vector_exactly() {
    for vector in vectors() {
        let name = str_of(&vector, "name");
        let message = message_of(&vector);
        let escape = str_of(&vector, "escape");
        assert_eq!(encode(&message), escape, "{name}: encoding");
        assert_eq!(
            decode(strip_frame(escape)).as_ref(),
            Ok(&message),
            "{name}: decoding"
        );
    }
}

#[test]
fn test_client_sh_writes_every_vector_it_can_express() {
    if !bash_available() {
        eprintln!("skipping: bash not installed");
        return;
    }
    for vector in vectors() {
        let name = str_of(&vector, "name");
        let escape = str_of(&vector, "escape");
        let id = id_of(&vector).0;
        match str_of(&vector, "verb") {
            // The shell client carries no presentation hints.
            "emit" if vector["bundle"].get("meta").is_some() => continue,
            "emit" => {
                let mime = vector["bundle"]["mime"].as_object().expect("mime map");
                let (kind, value) = mime
                    .iter()
                    .find(|(kind, _)| kind.as_str() != "text/plain")
                    .expect("a rich representation");
                let fallback = mime["text/plain"].as_str().expect("a fallback");
                // The shell's first emit is its sequence 1; the vector's id
                // gives way to the one its pid makes.
                let printed = run_client_sh(&format!(
                    "winter_emit_text {} {} {} {}; printf '\\n%s' \"$$\"",
                    shell_quote(kind),
                    shell_quote(value.as_str().expect("a text payload")),
                    shell_quote(fallback),
                    shell_quote(str_of(&vector, "trust")),
                ));
                let (written, pid) = printed.rsplit_once('\n').expect("the pid line");
                let pid: u32 = pid.parse().expect("a pid");
                let shell_id = BlockId::process_scoped(pid, 1).0;
                assert_eq!(shell_id >> BLOCK_SEQUENCE_BITS, u64::from(pid));
                let expected = escape.replace(&format!("id={id},"), &format!("id={shell_id},"));
                assert_eq!(written, expected, "{name}");
            }
            "open" => {
                let spec = serde_json::to_string(&vector["spec"]).expect("spec JSON");
                let written = run_client_sh(&format!(
                    "winter_open_block {id} {} {}",
                    shell_quote(str_of(&vector, "mime")),
                    shell_quote(&spec),
                ));
                assert_eq!(written, escape, "{name}");
            }
            "patch" => {
                let patch = serde_json::to_string(&vector["patch"]).expect("patch JSON");
                let written =
                    run_client_sh(&format!("winter_patch_block {id} {}", shell_quote(&patch)));
                assert_eq!(written, escape, "{name}");
            }
            "close" => {
                assert_eq!(
                    run_client_sh(&format!("winter_close_block {id}")),
                    escape,
                    "{name}"
                );
            }
            other => panic!("unknown verb {other}"),
        }
    }
}

#[test]
fn test_client_sh_scopes_a_live_block_s_id_by_its_process() {
    // Taken as it was given, two scripts both opening block 1 opened one:
    // the second was refused and its patches landed on the first's.
    if !bash_available() {
        return;
    }
    let open = |pid: &str| run_client_sh_as("winter_open_block 1 text/plain '\"a\"'", pid);
    let wire = BlockId::process_scoped(4242, 1).0;
    assert!(open("4242").contains(&format!(";id={wire},")), "{}", open("4242"));
    assert_ne!(open("4242"), open("4243"), "another process, another block");
    let patched = run_client_sh_as("winter_patch_block 1 '[]'", "4242");
    assert!(patched.contains(&format!(";id={wire};")), "the patch finds the block it opened");
}
