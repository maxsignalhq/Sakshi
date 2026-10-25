use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::mpsc;

use crate::event::Event;

/// Fans every published event out to all subscribers, in order.
///
/// Cheap to clone; clones share subscribers. Each subscriber has its own
/// unbounded queue, so a slow one never loses events or blocks the others.
#[derive(Clone, Default)]
pub struct EventBus {
    subscribers: Arc<Mutex<Vec<mpsc::UnboundedSender<Event>>>>,
}

impl EventBus {
    /// A bus with no subscribers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Start receiving every event published from now on.
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<Event> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.lock().push(tx);
        rx
    }

    /// Deliver `event` to every live subscriber and forget the dropped ones.
    pub fn publish(&self, event: &Event) {
        self.lock().retain(|tx| tx.send(event.clone()).is_ok());
    }

    /// Number of subscribers still listening.
    pub fn subscriber_count(&self) -> usize {
        self.lock().iter().filter(|tx| !tx.is_closed()).count()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<mpsc::UnboundedSender<Event>>> {
        // A panic while holding the lock cannot leave the list inconsistent.
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventKind;
    use crate::session::SessionId;

    fn message(text: &str) -> Event {
        Event::now(SessionId(1), EventKind::Message { text: text.into() })
    }

    fn text(event: Event) -> String {
        match event.kind {
            EventKind::Message { text } => text,
            other => format!("{other:?}"),
        }
    }

    #[test]
    fn every_subscriber_gets_every_event_in_order() {
        let bus = EventBus::new();
        let mut a = bus.subscribe();
        let mut b = bus.subscribe();
        bus.publish(&message("one"));
        bus.publish(&message("two"));
        for rx in [&mut a, &mut b] {
            assert_eq!(text(rx.try_recv().unwrap()), "one");
            assert_eq!(text(rx.try_recv().unwrap()), "two");
            assert!(rx.try_recv().is_err());
        }
    }

    #[test]
    fn subscribers_only_see_later_events() {
        let bus = EventBus::new();
        bus.publish(&message("early"));
        let mut rx = bus.subscribe();
        bus.publish(&message("late"));
        assert_eq!(text(rx.try_recv().unwrap()), "late");
    }

    #[test]
    fn dropped_subscribers_are_forgotten() {
        let bus = EventBus::new();
        let keep = bus.subscribe();
        let gone = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 2);
        drop(gone);
        assert_eq!(bus.subscriber_count(), 1);
        bus.publish(&message("x"));
        assert_eq!(bus.subscriber_count(), 1);
        drop(keep);
        bus.publish(&message("y"));
        assert_eq!(bus.subscriber_count(), 0);
    }

    #[test]
    fn publishing_with_no_subscribers_is_fine() {
        EventBus::new().publish(&message("nobody listens"));
    }

    #[test]
    fn clones_share_subscribers() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let clone = bus.clone();
        clone.publish(&message("via clone"));
        assert_eq!(text(rx.try_recv().unwrap()), "via clone");
    }
}
