//! Running the window: bootstrap on misty's `init`, construction and quitting,
//! restoring a saved session, misty's event loop, slow work run off the
//! event-loop thread, and waking the loop from the threads that feed it.

pub(in crate::window) mod event_loop;
pub(in crate::window) mod handle;
pub(crate) mod init;
pub(crate) mod jobs;
pub(crate) mod lifecycle;
pub(in crate::window) mod session_restore;
