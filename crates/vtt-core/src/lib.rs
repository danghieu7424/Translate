//! Lossless WebVTT parsing and serialization.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    line: usize,
    message: String,
}

impl ParseError {
    fn new(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }

    pub fn line(&self) -> usize {
        self.line
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid WebVTT at line {}: {}",
            self.line, self.message
        )
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineTagMismatch;

impl fmt::Display for InlineTagMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("replacement payload must preserve inline tags exactly and in order")
    }
}

impl std::error::Error for InlineTagMismatch {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VttCue {
    id: usize,
    identifier: Option<String>,
    start_time: String,
    end_time: String,
    settings: Option<String>,
    payload: String,
    speaker: Option<String>,
    raw_prefix: String,
    raw_suffix: String,
    newline: String,
}

impl VttCue {
    pub fn id(&self) -> usize {
        self.id
    }

    pub fn identifier(&self) -> Option<&str> {
        self.identifier.as_deref()
    }

    pub fn start_time(&self) -> &str {
        &self.start_time
    }

    pub fn end_time(&self) -> &str {
        &self.end_time
    }

    pub fn settings(&self) -> Option<&str> {
        self.settings.as_deref()
    }

    pub fn payload(&self) -> &str {
        &self.payload
    }

    pub fn speaker(&self) -> Option<&str> {
        self.speaker.as_deref()
    }

    pub fn inline_tags(&self) -> Vec<&str> {
        collect_inline_tags(&self.payload)
    }

    pub fn set_payload(&mut self, payload: &str) -> Result<(), InlineTagMismatch> {
        if collect_inline_tags(&self.payload) != collect_inline_tags(payload) {
            return Err(InlineTagMismatch);
        }

        self.payload = normalize_line_endings(payload, &self.newline);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Raw(String),
    Cue(VttCue),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VttDocument {
    segments: Vec<Segment>,
}

impl VttDocument {
    pub fn parse(source: &str) -> Result<Self, ParseError> {
        let lines = split_lines(source);
        let Some(header) = lines.first() else {
            return Err(ParseError::new(1, "missing WEBVTT header"));
        };

        let header_text = line_text(source, header);
        let header_text = header_text.strip_prefix('\u{feff}').unwrap_or(header_text);
        if !is_webvtt_header(header_text) {
            return Err(ParseError::new(1, "expected WEBVTT header"));
        }

        let mut segments = Vec::new();
        let mut raw_start = 0;
        let mut line_index = 1;
        let mut cue_id = 0;

        while line_index < lines.len() {
            let current_text = line_text(source, &lines[line_index]);

            if is_blank(current_text) {
                line_index += 1;
                continue;
            }

            if is_opaque_block_start(current_text) {
                line_index += 1;
                while line_index < lines.len() && !is_blank(line_text(source, &lines[line_index])) {
                    line_index += 1;
                }
                continue;
            }

            let (cue_start_index, timing_index, timing) =
                if let Some(timing) = parse_timing_line(current_text) {
                    (line_index, line_index, timing)
                } else if line_index + 1 < lines.len() {
                    let next_text = line_text(source, &lines[line_index + 1]);
                    if let Some(timing) = parse_timing_line(next_text) {
                        (line_index, line_index + 1, timing)
                    } else {
                        if current_text.contains("-->") {
                            return Err(ParseError::new(line_index + 1, "invalid cue timing line"));
                        }
                        line_index += 1;
                        continue;
                    }
                } else {
                    if current_text.contains("-->") {
                        return Err(ParseError::new(line_index + 1, "invalid cue timing line"));
                    }
                    line_index += 1;
                    continue;
                };

            let cue_start = lines[cue_start_index].start;
            let timing_line = &lines[timing_index];
            let payload_start = timing_line.end;
            let mut body_end_index = timing_index + 1;
            while body_end_index < lines.len()
                && !is_blank(line_text(source, &lines[body_end_index]))
            {
                body_end_index += 1;
            }

            let has_payload = body_end_index > timing_index + 1;
            let (payload_end, cue_end) = if has_payload {
                let last_body_line = &lines[body_end_index - 1];
                (last_body_line.content_end, last_body_line.end)
            } else {
                (payload_start, payload_start)
            };

            if raw_start < cue_start {
                segments.push(Segment::Raw(source[raw_start..cue_start].to_owned()));
            }

            let payload = source[payload_start..payload_end].to_owned();
            let speaker = extract_speaker(&payload);
            let identifier = if cue_start_index == timing_index {
                None
            } else {
                Some(line_text(source, &lines[cue_start_index]).to_owned())
            };
            let newline = payload
                .bytes()
                .position(|byte| byte == b'\n' || byte == b'\r')
                .map(|index| line_ending_at(&payload, index).to_owned())
                .filter(|newline| !newline.is_empty())
                .or_else(|| {
                    let newline = line_ending_at(source, timing_line.content_end);
                    (!newline.is_empty()).then(|| newline.to_owned())
                })
                .unwrap_or_else(|| "\n".to_owned());

            cue_id += 1;
            segments.push(Segment::Cue(VttCue {
                id: cue_id,
                identifier,
                start_time: timing.start,
                end_time: timing.end,
                settings: timing.settings,
                payload,
                speaker,
                raw_prefix: source[cue_start..payload_start].to_owned(),
                raw_suffix: source[payload_end..cue_end].to_owned(),
                newline,
            }));

            raw_start = cue_end;
            line_index = body_end_index;
        }

        if raw_start < source.len() {
            segments.push(Segment::Raw(source[raw_start..].to_owned()));
        }

        Ok(Self { segments })
    }

    pub fn cues(&self) -> impl Iterator<Item = &VttCue> {
        self.segments.iter().filter_map(|segment| match segment {
            Segment::Cue(cue) => Some(cue),
            Segment::Raw(_) => None,
        })
    }

    pub fn cue(&self, id: usize) -> Option<&VttCue> {
        self.cues().find(|cue| cue.id == id)
    }

    pub fn cue_mut(&mut self, id: usize) -> Option<&mut VttCue> {
        self.segments.iter_mut().find_map(|segment| match segment {
            Segment::Cue(cue) if cue.id == id => Some(cue),
            Segment::Raw(_) | Segment::Cue(_) => None,
        })
    }

    pub fn serialize(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for VttDocument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for segment in &self.segments {
            match segment {
                Segment::Raw(raw) => formatter.write_str(raw)?,
                Segment::Cue(cue) => {
                    formatter.write_str(&cue.raw_prefix)?;
                    formatter.write_str(&cue.payload)?;
                    formatter.write_str(&cue.raw_suffix)?;
                }
            }
        }
        Ok(())
    }
}

struct Timing {
    start: String,
    end: String,
    settings: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct Line {
    start: usize,
    content_end: usize,
    end: usize,
}

fn split_lines(source: &str) -> Vec<Line> {
    let bytes = source.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;

    while start < bytes.len() {
        let mut content_end = start;
        while content_end < bytes.len() && !matches!(bytes[content_end], b'\r' | b'\n') {
            content_end += 1;
        }

        let end = if content_end == bytes.len() {
            content_end
        } else if bytes[content_end] == b'\r' && bytes.get(content_end + 1).copied() == Some(b'\n')
        {
            content_end + 2
        } else {
            content_end + 1
        };

        lines.push(Line {
            start,
            content_end,
            end,
        });
        start = end;
    }

    lines
}

fn line_text<'a>(source: &'a str, line: &Line) -> &'a str {
    &source[line.start..line.content_end]
}

fn line_ending_at(source: &str, content_end: usize) -> &str {
    match source.as_bytes().get(content_end) {
        Some(b'\r') if source.as_bytes().get(content_end + 1) == Some(&b'\n') => "\r\n",
        Some(b'\r') => "\r",
        Some(b'\n') => "\n",
        _ => "",
    }
}

fn is_webvtt_header(line: &str) -> bool {
    line.strip_prefix("WEBVTT")
        .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with([' ', '\t']))
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn is_opaque_block_start(line: &str) -> bool {
    line == "STYLE"
        || line == "REGION"
        || line == "NOTE"
        || line
            .strip_prefix("NOTE")
            .is_some_and(|suffix| suffix.starts_with([' ', '\t']))
}

fn parse_timing_line(line: &str) -> Option<Timing> {
    let (start, end_and_settings) = line.split_once("-->")?;
    if end_and_settings.contains("-->") {
        return None;
    }

    let start = start.trim();
    let end_and_settings = end_and_settings.trim_start();
    let end_length = end_and_settings
        .find(char::is_whitespace)
        .unwrap_or(end_and_settings.len());
    let end = &end_and_settings[..end_length];
    if !valid_timestamp(start) || !valid_timestamp(end) {
        return None;
    }

    let settings = end_and_settings[end_length..].trim();
    Some(Timing {
        start: start.to_owned(),
        end: end.to_owned(),
        settings: (!settings.is_empty()).then(|| settings.to_owned()),
    })
}

fn valid_timestamp(timestamp: &str) -> bool {
    let Some((clock, milliseconds)) = timestamp.split_once('.') else {
        return false;
    };
    if milliseconds.len() != 3 || !milliseconds.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }

    let fields: Vec<&str> = clock.split(':').collect();
    match fields.as_slice() {
        [minutes, seconds] => {
            minutes.len() >= 2
                && minutes.bytes().all(|byte| byte.is_ascii_digit())
                && valid_second(seconds)
        }
        [hours, minutes, seconds] => {
            hours.len() >= 2
                && hours.bytes().all(|byte| byte.is_ascii_digit())
                && minutes.len() == 2
                && minutes.bytes().all(|byte| byte.is_ascii_digit())
                && minutes.parse::<u8>().is_ok_and(|value| value <= 59)
                && valid_second(seconds)
        }
        _ => false,
    }
}

fn valid_second(seconds: &str) -> bool {
    seconds.len() == 2
        && seconds.bytes().all(|byte| byte.is_ascii_digit())
        && seconds.parse::<u8>().is_ok_and(|value| value <= 59)
}

fn extract_speaker(payload: &str) -> Option<String> {
    collect_inline_tags(payload).into_iter().find_map(|tag| {
        let contents = &tag[1..tag.len() - 1];
        let annotation = contents.strip_prefix('v')?;
        if !annotation.starts_with([' ', '\t']) {
            return None;
        }

        let speaker = annotation.trim();
        (!speaker.is_empty()).then(|| speaker.to_owned())
    })
}

fn collect_inline_tags(payload: &str) -> Vec<&str> {
    let mut tags = Vec::new();
    let mut search_from = 0;

    while let Some(open_relative) = payload[search_from..].find('<') {
        let open = search_from + open_relative;
        let Some(close_relative) = payload[open + 1..].find('>') else {
            break;
        };
        let close = open + 1 + close_relative;
        let contents = &payload[open + 1..close];
        let name = contents.strip_prefix('/').unwrap_or(contents);
        if name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        {
            tags.push(&payload[open..=close]);
        }
        search_from = close + 1;
    }

    tags
}

fn normalize_line_endings(text: &str, newline: &str) -> String {
    let newline = if newline.is_empty() { "\n" } else { newline };
    let mut normalized = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(character) = chars.next() {
        match character {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                normalized.push_str(newline);
            }
            '\n' => normalized.push_str(newline),
            other => normalized.push(other),
        }
    }

    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn round_trip_preserves_header_note_identifier_settings_and_multiline_payload() {
        let source = concat!(
            "WEBVTT - sample\n",
            "Kind: captions\n",
            "Language: en\n",
            "\n",
            "NOTE generated by test\n",
            "This is not a cue.\n",
            "\n",
            "dialogue-1\n",
            "00:00:01.000 --> 00:00:02.500 align:start position:0%\n",
            "<v Speaker><i>First line</i>\n",
            "Second line</v>\n",
            "\n",
        );

        let document = VttDocument::parse(source).unwrap();
        assert_eq!(document.serialize(), source);

        let cue = document.cue(1).unwrap();
        assert_eq!(cue.identifier(), Some("dialogue-1"));
        assert_eq!(cue.start_time(), "00:00:01.000");
        assert_eq!(cue.end_time(), "00:00:02.500");
        assert_eq!(cue.settings(), Some("align:start position:0%"));
        assert_eq!(cue.speaker(), Some("Speaker"));
        assert_eq!(
            cue.payload(),
            "<v Speaker><i>First line</i>\nSecond line</v>"
        );
    }

    #[test]
    fn round_trip_preserves_crlf_and_cues_without_identifiers() {
        let source = concat!(
            "WEBVTT\r\n",
            "\r\n",
            "00:00:00.000 --> 00:00:01.000\r\n",
            "one\r\n",
            "two\r\n",
            "\r\n",
            "00:00:01.000 --> 00:00:02.000\r\n",
            "three\r\n",
            "\r\n",
        );

        let document = VttDocument::parse(source).unwrap();
        assert_eq!(document.serialize(), source);
        assert_eq!(document.cues().count(), 2);
        assert_eq!(document.cue(2).unwrap().identifier(), None);
    }

    #[test]
    fn round_trip_preserves_style_and_region_blocks() {
        let source = concat!(
            "WEBVTT\n",
            "\n",
            "STYLE\n",
            "::cue { color: lime; }\n",
            "\n",
            "REGION\n",
            "id:scroll\n",
            "width:40%\n",
            "\n",
            "00:00:00.000 --> 00:00:01.000\n",
            "text\n",
            "\n",
        );

        let document = VttDocument::parse(source).unwrap();
        assert_eq!(document.serialize(), source);
        assert_eq!(document.cues().count(), 1);
    }

    #[test]
    fn round_trip_preserves_bom_and_inline_formatting_tags() {
        let source = concat!(
            "\u{feff}WEBVTT\n",
            "\n",
            "00:00:00.000 --> 00:00:01.000\n",
            "<v Speaker><c.color><b>Bold</b></c> <i>italic</i></v>\n",
            "\n",
        );

        let document = VttDocument::parse(source).unwrap();
        assert_eq!(document.serialize(), source);
        assert_eq!(
            document.cue(1).unwrap().inline_tags(),
            vec![
                "<v Speaker>",
                "<c.color>",
                "<b>",
                "</b>",
                "</c>",
                "<i>",
                "</i>",
                "</v>"
            ]
        );
        assert_eq!(document.cue(1).unwrap().speaker(), Some("Speaker"));
    }

    #[test]
    fn round_trip_preserves_final_bytes_without_a_trailing_newline() {
        let source = "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nlast line";
        let document = VttDocument::parse(source).unwrap();
        assert_eq!(document.serialize(), source);
    }

    #[test]
    fn payload_replacement_preserves_tags_and_source_line_endings() {
        let source = concat!(
            "WEBVTT\r\n",
            "\r\n",
            "00:00:00.000 --> 00:00:01.000\r\n",
            "<v Mai><i>Hello</i>\r\n",
            "there</v>\r\n",
            "\r\n",
        );
        let mut document = VttDocument::parse(source).unwrap();

        document
            .cue_mut(1)
            .unwrap()
            .set_payload("<v Mai><i>Xin chào</i>\nnhé</v>")
            .unwrap();
        assert_eq!(
            document.serialize(),
            concat!(
                "WEBVTT\r\n",
                "\r\n",
                "00:00:00.000 --> 00:00:01.000\r\n",
                "<v Mai><i>Xin chào</i>\r\n",
                "nhé</v>\r\n",
                "\r\n",
            )
        );
    }

    #[test]
    fn payload_replacement_rejects_missing_or_changed_tags() {
        let source = "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\n<i>hello</i>\n";
        let mut document = VttDocument::parse(source).unwrap();
        let cue = document.cue_mut(1).unwrap();

        assert_eq!(cue.set_payload("xin chào"), Err(InlineTagMismatch));
        assert_eq!(cue.set_payload("<b>xin chào</b>"), Err(InlineTagMismatch));
        assert_eq!(cue.payload(), "<i>hello</i>");
    }

    #[test]
    fn malformed_header_and_timing_report_source_line() {
        let header_error = VttDocument::parse("not VTT\n").unwrap_err();
        assert_eq!(header_error.line(), 1);

        let timing_error =
            VttDocument::parse("WEBVTT\n\n00:00:00.00 --> 00:00:01.000\ntext\n").unwrap_err();
        assert_eq!(timing_error.line(), 3);
    }

    proptest! {
        #[test]
        fn arbitrary_multiline_payload_round_trips(lines in prop::collection::vec("[^\\r\\n]{1,64}", 1..6)) {
            let payload = lines.join("\r\n");
            let source = format!(
                "WEBVTT\r\n\r\n00:00:00.000 --> 00:00:01.000\r\n{payload}\r\n\r\n"
            );
            let document = VttDocument::parse(&source).unwrap();
            prop_assert_eq!(document.serialize(), source);
        }
    }
}
