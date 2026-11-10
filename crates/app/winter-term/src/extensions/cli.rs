//! `winter extension …`: installing extensions as files, from a terminal, and
//! what they may reach.
//!
//! ```text
//! winter extension install <file.wasm>
//! winter extension list
//! winter extension permissions <id>
//! winter extension grant <id> read|write|run <what>
//! winter extension revoke <id> read|write|run <what>
//! winter extension remove <id>
//! ```

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use super::manage::{self, Installed, Places, Review, Standing};
use super::package::GrantKind;

// ========================================================================
// Constants
// ========================================================================

/// How it is used, said when it is not.
const USAGE: &str = "usage: winter extension install <file.wasm> | list | permissions <id> | \
                     grant <id> read|write|run <what> | revoke <id> read|write|run <what> | \
                     remove <id>";

/// Said after any change to what an extension reaches.
const RESTART: &str = "Restart Winter for this to take effect.";

/// The answer that approves; any other refuses.
const YES: &str = "y";

/// How each grant is shown under its extension.
const INDENT: &str = "    ";

// ========================================================================
// Data Structures
// ========================================================================

/// One `winter extension` command.
#[derive(Debug, Eq, PartialEq)]
enum ExtensionCommand {
    Grant(GrantChange),
    Install(PathBuf),
    List,
    Permissions(String),
    Remove(String),
    Revoke(GrantChange),
}

/// A grant to give or take back.
#[derive(Debug, Eq, PartialEq)]
struct GrantChange {
    id: String,
    kind: GrantKind,
    value: String,
}

// ========================================================================
// Functions
// ========================================================================

/// Do the `winter extension` command `args` spell.
pub fn run(args: &[String]) -> ExitCode {
    let done = parse(args).and_then(|command| {
        let places = Places::user().ok_or("there is no state directory to keep approvals in")?;
        carry_out(command, &places)
    });
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("winter: {why}");
            ExitCode::FAILURE
        }
    }
}

fn parse(args: &[String]) -> Result<ExtensionCommand, String> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let change = |id: &str, kind: &str, value: &str| -> Result<GrantChange, String> {
        Ok(GrantChange {
            id: id.to_string(),
            kind: kind.parse()?,
            value: value.to_string(),
        })
    };
    match words.as_slice() {
        ["install", file] => Ok(ExtensionCommand::Install(PathBuf::from(file))),
        ["list"] => Ok(ExtensionCommand::List),
        ["permissions", id] => Ok(ExtensionCommand::Permissions(id.to_string())),
        ["grant", id, kind, value] => Ok(ExtensionCommand::Grant(change(id, kind, value)?)),
        ["revoke", id, kind, value] => Ok(ExtensionCommand::Revoke(change(id, kind, value)?)),
        ["remove", id] => Ok(ExtensionCommand::Remove(id.to_string())),
        _ => Err(USAGE.to_string()),
    }
}

fn carry_out(command: ExtensionCommand, places: &Places) -> Result<(), String> {
    match command {
        ExtensionCommand::Grant(change) => {
            let fresh = manage::grant(places, &change.id, change.kind, &change.value)?;
            say_changed(
                fresh,
                &format!("{} already has {}", change.id, change.value),
            );
        }
        ExtensionCommand::Install(file) => install(&file, places)?,
        ExtensionCommand::List => {
            let installed = manage::installed(places)?;
            if installed.is_empty() {
                println!("No extension is installed.");
            }
            installed.iter().for_each(print_installed);
        }
        ExtensionCommand::Permissions(id) => {
            let installed = manage::installed(places)?;
            let found = installed.iter().find(|one| one.id == id);
            print_installed(found.ok_or_else(|| format!("{id} is not installed"))?);
        }
        ExtensionCommand::Remove(id) => {
            manage::remove(places, &id)?;
            println!("Removed {id}. {RESTART}");
        }
        ExtensionCommand::Revoke(change) => {
            let had = manage::revoke(places, &change.id, change.kind, &change.value)?;
            say_changed(
                had,
                &format!("{} was not granted {}", change.id, change.value),
            );
        }
    }
    Ok(())
}

/// Show what `file` asks, and install it once that is approved.
fn install(file: &std::path::Path, places: &Places) -> Result<(), String> {
    let review = manage::review(file, places)?;
    println!("{}", review.package.heading());
    if let Some(description) = &review.package.description {
        println!("{INDENT}{description}");
    }
    let approved = match review.new.is_empty() && review.previous.is_some() {
        // Nothing it asks is new: an update the user approved before.
        true => true,
        false => ask_approval(&review)?,
    };
    if !approved {
        return Err(format!("{} was not installed", review.package.id));
    }
    let id = review.package.id.clone();
    let path = manage::install(review, places)?;
    println!("Installed {id} to {}. {RESTART}", path.display());
    Ok(())
}

/// Show what `review` would grant anew, and ask whether to.
fn ask_approval(review: &Review) -> Result<bool, String> {
    let heading = match review.previous {
        Some(_) => "It asks for more than you approved before:",
        None => "It asks to reach, beyond its own directory:",
    };
    println!("{heading}");
    let asks = review.new.lines();
    if asks.is_empty() {
        println!("{INDENT}nothing");
    }
    for line in asks {
        println!("{INDENT}{line}");
    }
    print!("Install it? [y/N] ");
    io::stdout().flush().map_err(|error| error.to_string())?;
    let mut answer = String::new();
    io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(|error| error.to_string())?;
    Ok(answer.trim().eq_ignore_ascii_case(YES))
}

fn print_installed(installed: &Installed) {
    let standing = match installed.standing {
        Standing::Approved => "approved",
        Standing::Changed => "changed since approved: not run",
        Standing::NotApproved => "not approved: not run",
    };
    println!("{} ({standing})", installed.id);
    let Some(approval) = &installed.approval else {
        return;
    };
    let granted = approval.granted.lines();
    if granted.is_empty() {
        println!("{INDENT}nothing beyond its own directory");
    }
    let added = approval.granted.beyond(&approval.asked).lines();
    for line in granted {
        let note = match added.contains(&line) {
            true => "  (added by you)",
            false => "",
        };
        println!("{INDENT}{line}{note}");
    }
    for line in approval.asked.beyond(&approval.granted).lines() {
        println!("{INDENT}{line}  (asked, not granted)");
    }
}

/// Say a change was made, or `unchanged` when there was none to make.
fn say_changed(changed: bool, unchanged: &str) {
    match changed {
        true => println!("Done. {RESTART}"),
        false => println!("{unchanged}."),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    #[test]
    fn test_grant_and_revoke_name_an_extension_a_kind_and_what_it_reaches() {
        assert_eq!(
            parse(&args(&["grant", "notes", "write", "~/Notes"])),
            Ok(ExtensionCommand::Grant(GrantChange {
                id: "notes".to_string(),
                kind: GrantKind::Write,
                value: "~/Notes".to_string(),
            }))
        );
        assert!(parse(&args(&["revoke", "notes", "network", "x"])).is_err());
        assert_eq!(parse(&args(&["install"])), Err(USAGE.to_string()));
    }
}
