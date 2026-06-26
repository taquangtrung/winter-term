//! Session size arbitration: a session shared by several clients sizes to
//! the smallest of them, and every attached client learns the result, so a
//! pane whose layout is larger than the session letterboxes its grid
//! instead of rendering a stream wrapped for a width it does not have.

use std::collections::HashMap;

use super::super::protocol::{CellPixels, Geometry, ServerMessage};
use super::super::session::{SessionId, SessionManager};
use super::dispatch::Client;

// ========================================================================
// Arbitration
// ========================================================================

/// Re-arbitrate `id`: when the effective size changed, resize the PTY and
/// tell every attached client the session's real geometry.
pub(super) fn sync_session_size(
    manager: &mut SessionManager,
    clients: &mut HashMap<u64, Client>,
    id: &SessionId,
) {
    if !manager.is_current(id) {
        return;
    }
    let Some(geometry) = effective_size(clients, id) else {
        return;
    };
    let (cols, rows) = (geometry.cols, geometry.rows);
    let grid_changed = manager.geometry(&id.name) != Some((cols, rows));
    if !grid_changed && manager.cell_px(&id.name) == geometry.cell_px {
        return;
    }
    if manager.resize(&id.name, geometry).is_err() {
        return;
    }
    // A change of cell size alone resizes the PTY's pixels but leaves the
    // grid every client draws as it was: nothing to tell them.
    if !grid_changed {
        return;
    }
    let msg = ServerMessage::Resized {
        session: id.name.clone(),
        cols,
        rows,
    };
    for client in clients.values_mut() {
        if client.is_attached(id) {
            client.enqueue(&msg);
        }
    }
}

/// The smallest geometry among the clients that reported one for `id`.
/// Clients that attached but never resized are unconstrained: they don't
/// shrink the session, and `None` means no client constrains it, so the
/// session keeps its current size (the behavior attach-only clients like
/// the CLI rely on). The cell size is the smallest each way among the
/// clients that reported one: the pixel size it gives never claims more
/// room than the smallest screen has.
fn effective_size(clients: &HashMap<u64, Client>, id: &SessionId) -> Option<Geometry> {
    clients
        .values()
        .filter_map(|client| client.wanted(id))
        .reduce(|smallest, wanted| Geometry {
            cell_px: smaller_cell(smallest.cell_px, wanted.cell_px),
            cols: smallest.cols.min(wanted.cols),
            rows: smallest.rows.min(wanted.rows),
        })
}

/// The smaller of two optional cell sizes in each dimension; an unknown one
/// defers to the known.
fn smaller_cell(a: Option<CellPixels>, b: Option<CellPixels>) -> Option<CellPixels> {
    match (a, b) {
        (Some(a), Some(b)) => Some(CellPixels {
            height: a.height.min(b.height),
            width: a.width.min(b.width),
        }),
        (known, None) | (None, known) => known,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::mux::server::connection::Outbox;

    fn grid(cols: u16, rows: u16) -> Geometry {
        Geometry {
            cell_px: None,
            cols,
            rows,
        }
    }

    fn id(name: &str) -> SessionId {
        SessionId {
            generation: 1,
            name: name.to_string(),
        }
    }

    /// A client constraining each listed session to the given geometry.
    fn client_wanting(wanted: &[(&str, u16, u16)]) -> Client {
        let (frames, _queued) = mpsc::channel();
        let mut client = Client::new(Outbox::new(frames));
        for &(name, cols, rows) in wanted {
            client.want(id(name), grid(cols, rows));
        }
        client
    }

    #[test]
    fn test_effective_cell_size_is_the_smallest_reported_each_way() {
        let cell = |width, height| Some(CellPixels { height, width });
        assert_eq!(smaller_cell(cell(9, 20), cell(10, 18)), cell(9, 18));
        assert_eq!(
            smaller_cell(None, cell(9, 20)),
            cell(9, 20),
            "unknown defers"
        );
        assert_eq!(smaller_cell(None, None), None);
    }

    #[test]
    fn test_effective_size_ignores_unconstrained_clients() {
        let mut clients = HashMap::new();
        clients.insert(1u64, client_wanting(&[("s", 120, 40)]));
        clients.insert(2u64, client_wanting(&[])); // attached, never resized
        clients.insert(3u64, client_wanting(&[("s", 80, 24), ("t", 50, 10)]));

        assert_eq!(effective_size(&clients, &id("s")), Some(grid(80, 24)));
        assert_eq!(effective_size(&clients, &id("t")), Some(grid(50, 10)));
        assert_eq!(effective_size(&clients, &id("missing")), None);
    }

    #[test]
    fn test_a_constraint_for_an_earlier_incarnation_does_not_apply() {
        let mut clients = HashMap::new();
        clients.insert(1u64, client_wanting(&[("s", 20, 5)]));
        let reused = SessionId {
            generation: 2,
            name: "s".to_string(),
        };
        assert_eq!(effective_size(&clients, &reused), None);
    }
}
