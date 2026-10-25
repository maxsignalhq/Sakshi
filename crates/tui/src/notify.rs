//! Desktop notifications, behind a trait so the rest of the UI never touches
//! the OS.

use std::thread;

/// Something that can tell the user a session needs them.
pub trait Notifier: Send {
    /// Show a notification. Must not block and must not fail loudly.
    fn notify(&mut self, title: &str, body: &str);
}

/// Native desktop notifications (macOS, Windows, Linux).
pub struct DesktopNotifier;

impl Notifier for DesktopNotifier {
    fn notify(&mut self, title: &str, body: &str) {
        let (title, body) = (title.to_string(), body.to_string());
        // Showing a notification can block on the desktop session, so it gets
        // its own short-lived thread. A failure just means no notification.
        let _ = thread::Builder::new()
            .name("sakshi-notify".into())
            .spawn(move || {
                if let Err(e) = notify_rust::Notification::new()
                    .appname("Sakshi")
                    .summary(&title)
                    .body(&body)
                    .show()
                {
                    tracing::debug!("desktop notification failed: {e}");
                }
            });
    }
}

/// Discards every notification.
pub struct NoopNotifier;

impl Notifier for NoopNotifier {
    fn notify(&mut self, _title: &str, _body: &str) {}
}
