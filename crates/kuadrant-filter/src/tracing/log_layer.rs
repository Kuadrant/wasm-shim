use tracing_subscriber::Layer;

pub(super) struct LogLayer;

impl<S> Layer<S> for LogLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let level = event.metadata().level();

        struct MessageVisitor(String);

        impl tracing::field::Visit for MessageVisitor {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0 = format!("{:?}", value);
                }
            }
        }

        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);

        match *level {
            tracing::Level::ERROR => log::error!("{}", visitor.0),
            tracing::Level::WARN => log::warn!("{}", visitor.0),
            tracing::Level::INFO => log::info!("{}", visitor.0),
            tracing::Level::DEBUG => log::debug!("{}", visitor.0),
            tracing::Level::TRACE => log::trace!("{}", visitor.0),
        }
    }
}
