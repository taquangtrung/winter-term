//! Notifications, from misty ([`misty::Notifier`]): what the app and its editors
//! report, how long each is shown, and what is kept afterwards. Re-exported
//! here so every editor reaches them through the model.

pub use misty::{
    LoggedNotice, Notice, NoticeAction, NoticeLog, Notifier, Severity, Toast, ToastStack,
};
