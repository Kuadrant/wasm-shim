use crate::v2::kuadrant::ReqRespCtx;
use core::time::Duration;
use sse_line_parser::RawEventLine;

mod sse_line_parser;

#[derive(Default)]
pub struct Event {
    /// The event name if given
    pub event: String,
    /// The event data
    pub data: String,
    /// The event id if given
    pub id: String,
    /// Retry duration if given
    pub retry: Option<Duration>,
}

#[derive(Default)]
pub struct EventParser {
    buffer: String,
    event_builder: EventBuilder,
}

impl EventParser {
    pub(crate) fn parse(&mut self, ctx: &mut ReqRespCtx) -> Result<Vec<Event>, String> {
        let chunk_bytes = ctx
            .get_http_response_body(0, ctx.body_size())
            .map_err(|e| e.to_string())?
            .unwrap_or(Vec::default());

        // taking advantage by the automatic deref coercion.
        // Because String implements Deref<Target=str>,
        // the compiler will automatically convert the string reference (i.e. &String) to a string slice - &str
        self.buffer
            .push_str(&String::from_utf8(chunk_bytes).map_err(|e| e.to_string())?);

        let mut events = Vec::default();

        while let Some(event) = self.parse_one_event()? {
            events.push(event);
        }

        return Ok(events);
    }

    fn parse_one_event(&mut self) -> Result<Option<Event>, String> {
        if self.buffer.is_empty() {
            return Ok(None);
        }
        loop {
            match sse_line_parser::line(self.buffer.as_ref()) {
                Ok((rem, next_line)) => {
                    self.event_builder.add(next_line);
                    let consumed = self.buffer.len() - rem.len();
                    let rem = self.buffer.split_off(consumed);
                    self.buffer = rem;
                    if self.event_builder.is_complete {
                        if let Some(event) = self.event_builder.dispatch() {
                            return Ok(Some(event));
                        }
                    }
                }
                Err(nom::Err::Incomplete(_)) => return Ok(None),
                Err(nom::Err::Error(err)) | Err(nom::Err::Failure(err)) => {
                    return Err(err.to_string())
                }
            }
        }
    }
}

#[derive(Default)]
struct EventBuilder {
    event: Event,
    is_complete: bool,
}

impl EventBuilder {
    fn add(&mut self, line: RawEventLine) {
        match line {
            RawEventLine::Field(field, val) => {
                let val = val.unwrap_or("");
                match field {
                    "event" => {
                        self.event.event = val.to_string();
                    }
                    "data" => {
                        if !self.event.data.is_empty() {
                            self.event.data.push('\u{000A}');
                        }
                        self.event.data.push_str(val);
                    }
                    "id" => {
                        if !val.contains('\u{0000}') {
                            self.event.id = val.to_string()
                        }
                    }
                    "retry" => {
                        if let Ok(val) = val.parse::<u64>() {
                            self.event.retry = Some(Duration::from_millis(val))
                        }
                    }
                    _ => {}
                }
            }
            RawEventLine::Comment(_) => {}
            RawEventLine::Empty => self.is_complete = true,
        }
    }

    fn dispatch(&mut self) -> Option<Event> {
        let builder = core::mem::take(self);
        let mut event = builder.event;

        if event.data.is_empty() {
            return None;
        }

        if sse_line_parser::is_lf(event.data.chars().next_back().unwrap()) {
            event.data.pop();
        }

        if event.event.is_empty() {
            event.event = "message".to_string();
        }

        Some(event)
    }
}
