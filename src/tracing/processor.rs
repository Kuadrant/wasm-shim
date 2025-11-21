use opentelemetry_sdk::{
    error::OTelSdkResult,
    trace::{Span, SpanData, SpanProcessor},
};
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

static SPAN_PROCESSOR: OnceLock<BufferingSpanProcessor> = OnceLock::new();

pub fn get_span_processor() -> &'static BufferingSpanProcessor {
    SPAN_PROCESSOR.get_or_init(BufferingSpanProcessor::new)
}

#[derive(Debug, Clone)]
pub struct SpanProcessorHandle;

impl SpanProcessor for SpanProcessorHandle {
    fn on_start(&self, span: &mut Span, cx: &opentelemetry::Context) {
        get_span_processor().on_start(span, cx)
    }

    fn on_end(&self, span: SpanData) {
        get_span_processor().on_end(span)
    }

    fn force_flush(&self) -> OTelSdkResult {
        get_span_processor().force_flush()
    }

    fn shutdown(&self) -> OTelSdkResult {
        get_span_processor().shutdown()
    }

    fn shutdown_with_timeout(&self, timeout: std::time::Duration) -> OTelSdkResult {
        get_span_processor().shutdown_with_timeout(timeout)
    }
}

#[derive(Debug)]
pub struct BufferingSpanProcessor {
    buffer: Mutex<VecDeque<SpanData>>,
    max_buffer_size: usize,
}

impl BufferingSpanProcessor {
    pub fn new() -> Self {
        //todo(adam-cattermole): what should our default capacity be?
        Self::with_capacity(100)
    }

    pub fn with_capacity(max_buffer_size: usize) -> Self {
        Self {
            buffer: Mutex::new(VecDeque::with_capacity(max_buffer_size)),
            max_buffer_size,
        }
    }

    /// Take all pending spans from the buffer, clearing it
    pub fn take_pending_spans(&self) -> Vec<SpanData> {
        let mut buffer = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *buffer).into()
    }

    /// Get the number of pending spans in the buffer
    pub fn pending_count(&self) -> usize {
        self.buffer.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Check if there are any pending spans
    pub fn has_pending_spans(&self) -> bool {
        !self
            .buffer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }

    /// Clear all pending spans without returning them
    pub fn clear(&self) {
        self.buffer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// Get the maximum buffer size
    pub fn max_buffer_size(&self) -> usize {
        self.max_buffer_size
    }
}

impl Default for BufferingSpanProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl SpanProcessor for BufferingSpanProcessor {
    fn on_start(&self, _span: &mut Span, _cx: &opentelemetry::Context) {
        // Nothing to do on span start
    }

    fn on_end(&self, span: SpanData) {
        let mut buffer = self.buffer.lock().unwrap_or_else(|e| e.into_inner());

        // FIFO
        if buffer.len() >= self.max_buffer_size {
            log::warn!(
                "Span buffer full ({}), dropping oldest span",
                self.max_buffer_size
            );
            buffer.pop_front();
        }

        buffer.push_back(span);
    }

    fn force_flush(&self) -> OTelSdkResult {
        // No-op: flushing happens via take_pending_spans()
        Ok(())
    }

    fn shutdown(&self) -> OTelSdkResult {
        self.clear();
        Ok(())
    }

    fn shutdown_with_timeout(&self, _timeout: std::time::Duration) -> OTelSdkResult {
        self.shutdown()
    }
}
