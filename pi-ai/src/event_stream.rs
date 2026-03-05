use tokio::sync::{mpsc, oneshot};

use crate::types::{AssistantMessage, AssistantMessageEvent, StopReason};

/// Generic event stream with push/end semantics and a final result.
/// T = event type, R = final result type.
pub struct EventStream<T, R> {
    tx: mpsc::UnboundedSender<T>,
    rx: mpsc::UnboundedReceiver<T>,
    result_tx: Option<oneshot::Sender<R>>,
    result_rx: Option<oneshot::Receiver<R>>,
}

impl<T, R> EventStream<T, R> {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let (result_tx, result_rx) = oneshot::channel();
        Self {
            tx,
            rx,
            result_tx: Some(result_tx),
            result_rx: Some(result_rx),
        }
    }

    /// Get a sender handle for pushing events.
    pub fn sender(&self) -> EventStreamSender<T, R> {
        EventStreamSender {
            tx: self.tx.clone(),
            result_tx: None, // Will be set via take_result_sender
        }
    }

    /// Take the result sender (can only be called once).
    pub fn take_result_sender(&mut self) -> Option<oneshot::Sender<R>> {
        self.result_tx.take()
    }

    /// Receive the next event.
    pub async fn recv(&mut self) -> Option<T> {
        self.rx.recv().await
    }

    /// Wait for the final result.
    pub async fn result(mut self) -> Option<R> {
        if let Some(rx) = self.result_rx.take() {
            rx.await.ok()
        } else {
            None
        }
    }
}

/// Sender half of an EventStream.
pub struct EventStreamSender<T, R> {
    tx: mpsc::UnboundedSender<T>,
    result_tx: Option<oneshot::Sender<R>>,
}

impl<T, R> EventStreamSender<T, R> {
    pub fn with_result_sender(mut self, result_tx: oneshot::Sender<R>) -> Self {
        self.result_tx = Some(result_tx);
        self
    }

    pub fn push(&self, event: T) {
        let _ = self.tx.send(event);
    }

    pub fn end(self, result: R) {
        if let Some(tx) = self.result_tx {
            let _ = tx.send(result);
        }
        // Dropping self.tx closes the channel
    }
}

impl<T, R> Clone for EventStreamSender<T, R> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            result_tx: None, // Only one sender can hold the result_tx
        }
    }
}

// ── AssistantMessageEventStream ─────────────────────────────────────────

pub struct AssistantMessageEventStream {
    inner: EventStream<AssistantMessageEvent, AssistantMessage>,
}

pub struct AssistantMessageEventSender {
    inner: EventStreamSender<AssistantMessageEvent, AssistantMessage>,
}

impl AssistantMessageEventStream {
    pub fn new() -> (Self, AssistantMessageEventSender) {
        let mut stream = EventStream::new();
        let result_tx = stream.take_result_sender().unwrap();
        let sender = stream.sender().with_result_sender(result_tx);
        (
            Self { inner: stream },
            AssistantMessageEventSender { inner: sender },
        )
    }

    pub async fn recv(&mut self) -> Option<AssistantMessageEvent> {
        self.inner.recv().await
    }

    pub async fn result(self) -> Option<AssistantMessage> {
        self.inner.result().await
    }
}

impl Default for AssistantMessageEventStream {
    fn default() -> Self {
        Self::new().0
    }
}

impl AssistantMessageEventSender {
    pub fn push(&self, event: AssistantMessageEvent) {
        self.inner.push(event);
    }

    pub fn done(self, message: AssistantMessage) {
        let reason = message.stop_reason;
        self.inner.push(AssistantMessageEvent::Done {
            reason,
            message: message.clone(),
        });
        self.inner.end(message);
    }

    pub fn error(self, error: AssistantMessage) {
        let reason = error.stop_reason;
        self.inner.push(AssistantMessageEvent::Error {
            reason,
            error: error.clone(),
        });
        self.inner.end(error);
    }

    pub fn finish(self, message: AssistantMessage) {
        match message.stop_reason {
            StopReason::Error | StopReason::Aborted => self.error(message),
            _ => self.done(message),
        }
    }
}
