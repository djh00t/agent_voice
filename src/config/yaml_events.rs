#[cfg(test)]
use std::cell::Cell;
use std::fmt;
use std::marker::PhantomPinned;
use std::mem::MaybeUninit;
use std::pin::Pin;

use anyhow::{Result, anyhow};
use unsafe_libyaml::{
    yaml_encoding_t::YAML_UTF8_ENCODING,
    yaml_event_delete, yaml_event_t,
    yaml_event_type_t::{
        YAML_ALIAS_EVENT, YAML_DOCUMENT_END_EVENT, YAML_DOCUMENT_START_EVENT,
        YAML_MAPPING_END_EVENT, YAML_MAPPING_START_EVENT, YAML_SCALAR_EVENT,
        YAML_SEQUENCE_END_EVENT, YAML_SEQUENCE_START_EVENT, YAML_STREAM_END_EVENT,
        YAML_STREAM_START_EVENT,
    },
    yaml_mapping_style_t::YAML_FLOW_MAPPING_STYLE,
    yaml_parser_delete, yaml_parser_initialize, yaml_parser_parse, yaml_parser_set_encoding,
    yaml_parser_set_input_string, yaml_parser_t,
    yaml_scalar_style_t::{
        YAML_DOUBLE_QUOTED_SCALAR_STYLE, YAML_FOLDED_SCALAR_STYLE, YAML_LITERAL_SCALAR_STYLE,
        YAML_PLAIN_SCALAR_STYLE, YAML_SINGLE_QUOTED_SCALAR_STYLE,
    },
    yaml_sequence_style_t::YAML_FLOW_SEQUENCE_STYLE,
};

const INITIALIZATION_ERROR: &str = "config YAML event reader: initialization_failed";
const PARSE_ERROR: &str = "config YAML event reader: parse_failed";

#[cfg(test)]
thread_local! {
    static EVENT_DELETE_COUNT: Cell<usize> = const { Cell::new(0) };
}

pub(super) struct YamlEventReader<'input> {
    state: Pin<Box<ParserState<'input>>>,
    finished: bool,
}

struct ParserState<'input> {
    parser: MaybeUninit<yaml_parser_t>,
    input: &'input [u8],
    initialized: bool,
    _pin: PhantomPinned,
}

impl Drop for ParserState<'_> {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: `initialized` is set only after libyaml initializes this stable allocation.
            unsafe { yaml_parser_delete(self.parser.as_mut_ptr()) };
        }
    }
}

#[cfg_attr(test, derive(PartialEq, Eq))]
pub(super) enum YamlEvent {
    StreamStart,
    StreamEnd,
    DocumentStart,
    DocumentEnd,
    MappingStart {
        flow: bool,
        anchored: bool,
        tagged: bool,
    },
    MappingEnd,
    SequenceStart {
        flow: bool,
        anchored: bool,
        tagged: bool,
    },
    SequenceEnd,
    Alias,
    Scalar {
        value: Box<[u8]>,
        style: YamlScalarStyle,
        anchored: bool,
        tagged: bool,
    },
}

impl fmt::Debug for YamlEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StreamStart => formatter.write_str("StreamStart"),
            Self::StreamEnd => formatter.write_str("StreamEnd"),
            Self::DocumentStart => formatter.write_str("DocumentStart"),
            Self::DocumentEnd => formatter.write_str("DocumentEnd"),
            Self::MappingStart {
                flow,
                anchored,
                tagged,
            } => formatter
                .debug_struct("MappingStart")
                .field("flow", flow)
                .field("anchored", anchored)
                .field("tagged", tagged)
                .finish(),
            Self::MappingEnd => formatter.write_str("MappingEnd"),
            Self::SequenceStart {
                flow,
                anchored,
                tagged,
            } => formatter
                .debug_struct("SequenceStart")
                .field("flow", flow)
                .field("anchored", anchored)
                .field("tagged", tagged)
                .finish(),
            Self::SequenceEnd => formatter.write_str("SequenceEnd"),
            Self::Alias => formatter.write_str("Alias"),
            Self::Scalar {
                value,
                style,
                anchored,
                tagged,
            } => formatter
                .debug_struct("Scalar")
                .field("length", &value.len())
                .field("style", style)
                .field("anchored", anchored)
                .field("tagged", tagged)
                .finish(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum YamlScalarStyle {
    Plain,
    SingleQuoted,
    DoubleQuoted,
    Literal,
    Folded,
}

impl<'input> YamlEventReader<'input> {
    pub(super) fn new(input: &'input str) -> Result<Self> {
        let input = input.as_bytes();
        let input_len = u64::try_from(input.len()).map_err(|_| parse_error())?;
        let mut state = Box::pin(ParserState {
            parser: MaybeUninit::uninit(),
            input,
            initialized: false,
            _pin: PhantomPinned,
        });
        let state_mut = unsafe {
            // SAFETY: this allocation is pinned before initialization and is not moved afterward.
            Pin::as_mut(&mut state).get_unchecked_mut()
        };
        let parser = state_mut.parser.as_mut_ptr();
        let initialized = unsafe {
            // SAFETY: `parser` points to the stable, uninitialized parser allocation owned above.
            yaml_parser_initialize(parser).ok
        };
        if !initialized {
            return Err(anyhow!(INITIALIZATION_ERROR));
        }
        state_mut.initialized = true;
        unsafe {
            // SAFETY: libyaml now owns only parser-internal state; `input` outlives `state`.
            yaml_parser_set_encoding(parser, YAML_UTF8_ENCODING);
            yaml_parser_set_input_string(parser, state_mut.input.as_ptr(), input_len);
        }
        Ok(Self {
            state,
            finished: false,
        })
    }

    pub(super) fn next(&mut self) -> Result<Option<YamlEvent>> {
        if self.finished {
            return Ok(None);
        }
        let (event, stream_end) = {
            let parser = unsafe {
                // SAFETY: the parser remains pinned and initialized for the reader's lifetime.
                Pin::as_mut(&mut self.state)
                    .get_unchecked_mut()
                    .parser
                    .as_mut_ptr()
            };
            let raw = EventGuard::parse(parser)?;
            convert_event(raw.event())?
        };
        self.finished = stream_end;
        Ok(Some(event))
    }
}

pub(super) fn validate_syntax(input: &str) -> Result<()> {
    let mut reader = YamlEventReader::new(input)?;
    while reader.next()?.is_some() {}
    Ok(())
}

struct EventGuard {
    event: MaybeUninit<yaml_event_t>,
}

impl EventGuard {
    fn parse(parser: *mut yaml_parser_t) -> Result<Self> {
        let mut event = MaybeUninit::uninit();
        let parsed = unsafe {
            // SAFETY: `parser` is initialized and `event` provides writable storage for libyaml.
            yaml_parser_parse(parser, event.as_mut_ptr()).ok
        };
        if !parsed {
            return Err(parse_error());
        }
        Ok(Self { event })
    }

    fn event(&self) -> &yaml_event_t {
        unsafe {
            // SAFETY: a successful `yaml_parser_parse` initializes the event before this guard exists.
            self.event.assume_init_ref()
        }
    }
}

impl Drop for EventGuard {
    fn drop(&mut self) {
        #[cfg(test)]
        EVENT_DELETE_COUNT.with(|count| count.set(count.get() + 1));
        unsafe {
            // SAFETY: this guard exists only for a successfully initialized libyaml event.
            yaml_event_delete(self.event.as_mut_ptr())
        };
    }
}

fn convert_event(event: &yaml_event_t) -> Result<(YamlEvent, bool)> {
    let converted = match event.type_ {
        YAML_STREAM_START_EVENT => YamlEvent::StreamStart,
        YAML_STREAM_END_EVENT => return Ok((YamlEvent::StreamEnd, true)),
        YAML_DOCUMENT_START_EVENT => YamlEvent::DocumentStart,
        YAML_DOCUMENT_END_EVENT => YamlEvent::DocumentEnd,
        YAML_ALIAS_EVENT => YamlEvent::Alias,
        YAML_MAPPING_END_EVENT => YamlEvent::MappingEnd,
        YAML_SEQUENCE_END_EVENT => YamlEvent::SequenceEnd,
        YAML_MAPPING_START_EVENT => unsafe {
            // SAFETY: libyaml initializes the matching union field for this event type.
            let mapping = event.data.mapping_start;
            YamlEvent::MappingStart {
                flow: mapping.style == YAML_FLOW_MAPPING_STYLE,
                anchored: !mapping.anchor.is_null(),
                tagged: !mapping.tag.is_null(),
            }
        },
        YAML_SEQUENCE_START_EVENT => unsafe {
            // SAFETY: libyaml initializes the matching union field for this event type.
            let sequence = event.data.sequence_start;
            YamlEvent::SequenceStart {
                flow: sequence.style == YAML_FLOW_SEQUENCE_STYLE,
                anchored: !sequence.anchor.is_null(),
                tagged: !sequence.tag.is_null(),
            }
        },
        YAML_SCALAR_EVENT => unsafe {
            // SAFETY: libyaml initializes the matching union field for this event type.
            let scalar = event.data.scalar;
            let length = usize::try_from(scalar.length).map_err(|_| parse_error())?;
            if scalar.value.is_null() && length != 0 {
                return Err(parse_error());
            }
            let value = if length == 0 {
                Box::default()
            } else {
                // SAFETY: non-null scalar storage is valid for the event's declared length until deletion.
                std::slice::from_raw_parts(scalar.value, length)
                    .to_vec()
                    .into_boxed_slice()
            };
            YamlEvent::Scalar {
                value,
                style: scalar_style(scalar.style)?,
                anchored: !scalar.anchor.is_null(),
                tagged: !scalar.tag.is_null(),
            }
        },
        _ => return Err(parse_error()),
    };
    Ok((converted, false))
}

fn scalar_style(style: unsafe_libyaml::yaml_scalar_style_t) -> Result<YamlScalarStyle> {
    match style {
        YAML_PLAIN_SCALAR_STYLE => Ok(YamlScalarStyle::Plain),
        YAML_SINGLE_QUOTED_SCALAR_STYLE => Ok(YamlScalarStyle::SingleQuoted),
        YAML_DOUBLE_QUOTED_SCALAR_STYLE => Ok(YamlScalarStyle::DoubleQuoted),
        YAML_LITERAL_SCALAR_STYLE => Ok(YamlScalarStyle::Literal),
        YAML_FOLDED_SCALAR_STYLE => Ok(YamlScalarStyle::Folded),
        _ => Err(parse_error()),
    }
}

fn parse_error() -> anyhow::Error {
    anyhow!(PARSE_ERROR)
}

#[cfg(test)]
mod tests {
    use super::{YamlEvent, YamlEventReader, YamlScalarStyle};
    use unsafe_libyaml::{
        yaml_event_type_t::YAML_SCALAR_EVENT, yaml_scalar_style_t::YAML_ANY_SCALAR_STYLE,
    };

    fn read(source: &str) -> Vec<YamlEvent> {
        let mut reader = YamlEventReader::new(source).expect("valid YAML reader");
        std::iter::from_fn(|| reader.next().transpose())
            .collect::<Result<Vec<_>, _>>()
            .expect("valid YAML events")
    }

    fn scalar(value: &[u8], style: YamlScalarStyle) -> YamlEvent {
        YamlEvent::Scalar {
            value: value.to_vec().into_boxed_slice(),
            style,
            anchored: false,
            tagged: false,
        }
    }

    #[test]
    fn event_reader_contract() {
        let source =
            "\"backup\": { 'retention_days': '7', \"max_age_hours\": \"24\" } # trailing\n";
        let events = read(source);

        assert_eq!(
            events,
            vec![
                YamlEvent::StreamStart,
                YamlEvent::DocumentStart,
                YamlEvent::MappingStart {
                    flow: false,
                    anchored: false,
                    tagged: false,
                },
                scalar(b"backup", YamlScalarStyle::DoubleQuoted),
                YamlEvent::MappingStart {
                    flow: true,
                    anchored: false,
                    tagged: false,
                },
                scalar(b"retention_days", YamlScalarStyle::SingleQuoted),
                scalar(b"7", YamlScalarStyle::SingleQuoted),
                scalar(b"max_age_hours", YamlScalarStyle::DoubleQuoted),
                scalar(b"24", YamlScalarStyle::DoubleQuoted),
                YamlEvent::MappingEnd,
                YamlEvent::MappingEnd,
                YamlEvent::DocumentEnd,
                YamlEvent::StreamEnd,
            ]
        );
    }

    #[test]
    fn emits_exact_events_for_indented_block_document() {
        let events = read("  backup:\n    retention_days: '7'\n    max_age_hours: \"24\"\n");

        assert_eq!(
            events,
            vec![
                YamlEvent::StreamStart,
                YamlEvent::DocumentStart,
                YamlEvent::MappingStart {
                    flow: false,
                    anchored: false,
                    tagged: false,
                },
                scalar(b"backup", YamlScalarStyle::Plain),
                YamlEvent::MappingStart {
                    flow: false,
                    anchored: false,
                    tagged: false,
                },
                scalar(b"retention_days", YamlScalarStyle::Plain),
                scalar(b"7", YamlScalarStyle::SingleQuoted),
                scalar(b"max_age_hours", YamlScalarStyle::Plain),
                scalar(b"24", YamlScalarStyle::DoubleQuoted),
                YamlEvent::MappingEnd,
                YamlEvent::MappingEnd,
                YamlEvent::DocumentEnd,
                YamlEvent::StreamEnd,
            ]
        );
    }

    #[test]
    fn preserves_scalar_styles_and_redacts_debug_values() {
        let events = read(
            "plain: bare\nsingle: 'one''s'\ndouble: \"comma,#\"\nliteral: |\n  line\nfolded: >\n  words\n",
        );
        let scalars = events
            .iter()
            .filter_map(|event| match event {
                YamlEvent::Scalar { value, style, .. } => {
                    Some((value.as_ref(), *style, format!("{event:?}")))
                }
                _ => None,
            })
            .collect::<Vec<_>>();

        assert!(
            scalars
                .iter()
                .any(|(value, style, _)| *value == b"bare" && *style == YamlScalarStyle::Plain)
        );
        assert!(
            scalars
                .iter()
                .any(|(value, style, _)| *value == b"one's"
                    && *style == YamlScalarStyle::SingleQuoted)
        );
        assert!(scalars.iter().any(
            |(value, style, _)| *value == b"comma,#" && *style == YamlScalarStyle::DoubleQuoted
        ));
        assert!(
            scalars
                .iter()
                .any(|(value, style, _)| *value == b"line\n" && *style == YamlScalarStyle::Literal)
        );
        assert!(
            scalars
                .iter()
                .any(|(value, style, _)| *value == b"words\n" && *style == YamlScalarStyle::Folded)
        );
        assert!(
            scalars
                .iter()
                .all(|(_, _, debug)| !debug.contains("one's") && !debug.contains("comma,#"))
        );
    }

    #[test]
    fn treats_apostrophes_commas_hashes_and_comments_as_parser_boundaries() {
        let events = read("backup: { policy: 'it''s, # data', other: mid'scalar } # comment\n");
        let values = events
            .iter()
            .filter_map(|event| match event {
                YamlEvent::Scalar { value, .. } => Some(value.as_ref()),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert!(values.contains(&b"it's, # data".as_ref()));
        assert!(values.contains(&b"mid'scalar".as_ref()));
        assert!(
            !values
                .iter()
                .any(|value| value.windows(7).any(|window| window == b"comment"))
        );
    }

    #[test]
    fn malformed_input_returns_only_the_fixed_redacted_error() {
        let error = super::validate_syntax("sentinel://secret: [").expect_err("malformed input");
        assert_eq!(error.to_string(), "config YAML event reader: parse_failed");
        assert!(!error.to_string().contains("sentinel"));
    }

    #[test]
    fn unsupported_scalar_style_returns_only_the_fixed_redacted_error() {
        let mut reader = YamlEventReader::new("sentinel://secret\n").expect("reader");
        let parser = unsafe {
            std::pin::Pin::as_mut(&mut reader.state)
                .get_unchecked_mut()
                .parser
                .as_mut_ptr()
        };
        let stream_start = super::EventGuard::parse(parser).expect("stream start");
        drop(stream_start);
        let document_start = super::EventGuard::parse(parser).expect("document start");
        drop(document_start);
        super::EVENT_DELETE_COUNT.with(|count| count.set(0));
        let mut scalar_guard = super::EventGuard::parse(parser).expect("scalar");
        assert_eq!(scalar_guard.event().type_, YAML_SCALAR_EVENT);
        unsafe {
            (*scalar_guard.event.as_mut_ptr()).data.scalar.style = YAML_ANY_SCALAR_STYLE;
        }

        let error = super::convert_event(scalar_guard.event()).expect_err("unsupported style");
        drop(scalar_guard);

        assert_eq!(error.to_string(), "config YAML event reader: parse_failed");
        assert!(!error.to_string().contains("sentinel://secret"));
        assert_eq!(super::EVENT_DELETE_COUNT.with(|count| count.get()), 1);
        drop(reader);
    }

    #[test]
    fn reader_can_be_dropped_after_parse_failure_without_disclosing_input() {
        let source = String::from("sentinel://secret: [");
        let mut reader = YamlEventReader::new(&source).expect("reader");
        let error = loop {
            match reader.next() {
                Ok(Some(_)) => continue,
                Ok(None) => panic!("malformed input unexpectedly ended"),
                Err(error) => break error,
            }
        };

        assert_eq!(error.to_string(), "config YAML event reader: parse_failed");
        assert!(!error.to_string().contains("sentinel://secret"));
        drop(reader);
        assert_eq!(source, "sentinel://secret: [");
    }

    #[test]
    fn reader_is_inert_after_stream_end_and_input_outlives_drop() {
        let source = String::from("key: value\n");
        let mut reader = YamlEventReader::new(&source).expect("reader");
        while reader.next().expect("event result").is_some() {}
        assert!(reader.next().expect("inert result").is_none());
        drop(reader);
        assert_eq!(source, "key: value\n");
    }
}
