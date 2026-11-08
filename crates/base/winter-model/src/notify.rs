//! Notifications, from [`winter_notify`]: what the app and its editors
//! report, how long each is shown, and what is kept afterwards. Re-exported
//! here so every editor reaches them through the model.

pub use winter_notify::{
    Notice, Notification, NotificationLog, Notifier, Severity, Toast, ToastStack,
};
