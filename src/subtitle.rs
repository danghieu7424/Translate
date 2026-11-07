use anyhow::{Context, Result};
use std::fs::File;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleFormat {
    Vtt,
    Srt,
}

impl SubtitleFormat {
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_string_lossy().to_lowercase();
        match ext.as_str() {
            "vtt" => Some(SubtitleFormat::Vtt),
            "srt" => Some(SubtitleFormat::Srt),
            _ => None,
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            SubtitleFormat::Vtt => "vtt",
            SubtitleFormat::Srt => "srt",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SubtitleCue {
    pub index: usize,
    pub start_ms: usize,
    pub end_ms: usize,
    pub settings: Option<String>,
    pub text: String,
}

/// Chuyển đổi timestamp dạng hh:mm:ss.mmm hoặc mm:ss.mmm sang millisecond
pub fn parse_timestamp(s: &str) -> Option<usize> {
    let clean = s.trim().replace(',', ".");
    let parts: Vec<&str> = clean.split(':').collect();
    if parts.len() == 3 {
        let h = parts[0].trim().parse::<usize>().ok()?;
        let m = parts[1].trim().parse::<usize>().ok()?;
        let sec_parts: Vec<&str> = parts[2].trim().split('.').collect();
        let s = sec_parts[0].trim().parse::<usize>().ok()?;
        let ms = if sec_parts.len() > 1 {
            let raw = sec_parts[1].trim();
            let padded = format!("{:0<3}", raw);
            padded[..3].parse::<usize>().ok()?
        } else {
            0
        };
        Some(h * 3_600_000 + m * 60_000 + s * 1_000 + ms)
    } else if parts.len() == 2 {
        let m = parts[0].trim().parse::<usize>().ok()?;
        let sec_parts: Vec<&str> = parts[1].trim().split('.').collect();
        let s = sec_parts[0].trim().parse::<usize>().ok()?;
        let ms = if sec_parts.len() > 1 {
            let raw = sec_parts[1].trim();
            let padded = format!("{:0<3}", raw);
            padded[..3].parse::<usize>().ok()?
        } else {
            0
        };
        Some(m * 60_000 + s * 1_000 + ms)
    } else {
        None
    }
}

/// Định dạng millisecond thành chuỗi VTT (dấu chấm) hoặc SRT (dấu phẩy)
pub fn format_timestamp(ms: usize, format: SubtitleFormat) -> String {
    let h = ms / 3_600_000;
    let m = (ms % 3_600_000) / 60_000;
    let s = (ms % 60_000) / 1_000;
    let millis = ms % 1_000;
    match format {
        SubtitleFormat::Vtt => format!("{:02}:{:02}:{:02}.{:03}", h, m, s, millis),
        SubtitleFormat::Srt => format!("{:02}:{:02}:{:02},{:03}", h, m, s, millis),
    }
}

/// Phân tích file phụ đề (WebVTT hoặc SRT) thành danh sách các SubtitleCue
pub fn parse_subtitle_file(path: &Path) -> Result<(SubtitleFormat, Vec<SubtitleCue>)> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("Không thể đọc file phụ đề: {:?}", path))?;
    let content = raw.trim_start_matches('\u{feff}'); // Xóa BOM UTF-8 nếu có

    let format = SubtitleFormat::from_path(path).unwrap_or_else(|| {
        if content.starts_with("WEBVTT") {
            SubtitleFormat::Vtt
        } else {
            SubtitleFormat::Srt
        }
    });

    let mut cues = Vec::new();
    let mut cur_index = 0usize;
    let mut cur_start = 0usize;
    let mut cur_end = 0usize;
    let mut cur_settings = None;
    let mut collecting = false;
    let mut text_lines = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed == "WEBVTT" || trimmed.starts_with("NOTE") {
            continue;
        }

        if trimmed.is_empty() {
            if collecting && !text_lines.is_empty() && cur_end > cur_start {
                cues.push(SubtitleCue {
                    index: if cur_index == 0 { cues.len() + 1 } else { cur_index },
                    start_ms: cur_start,
                    end_ms: cur_end,
                    settings: cur_settings.take(),
                    text: text_lines.join(" "),
                });
            }
            text_lines.clear();
            collecting = false;
            cur_index = 0;
            continue;
        }

        if trimmed.contains("-->") {
            let parts: Vec<&str> = trimmed.split("-->").collect();
            if parts.len() == 2 {
                let start_str = parts[0].trim();
                let end_and_settings = parts[1].trim();

                let mut end_parts = end_and_settings.split_whitespace();
                if let Some(end_str) = end_parts.next() {
                    if let (Some(s), Some(e)) = (parse_timestamp(start_str), parse_timestamp(end_str)) {
                        cur_start = s;
                        cur_end = e;
                        let remainder: Vec<&str> = end_parts.collect();
                        if !remainder.is_empty() {
                            cur_settings = Some(remainder.join(" "));
                        }
                        collecting = true;
                        text_lines.clear();
                        continue;
                    }
                }
            }
        }

        if !collecting {
            if let Ok(idx) = trimmed.parse::<usize>() {
                cur_index = idx;
                continue;
            }
        }

        if collecting {
            text_lines.push(trimmed.to_string());
        }
    }

    if collecting && !text_lines.is_empty() && cur_end > cur_start {
        cues.push(SubtitleCue {
            index: if cur_index == 0 { cues.len() + 1 } else { cur_index },
            start_ms: cur_start,
            end_ms: cur_end,
            settings: cur_settings,
            text: text_lines.join(" "),
        });
    }

    Ok((format, cues))
}

/// Ghi danh sách phụ đề đã dịch ra file
pub fn save_subtitle_file(
    path: &Path,
    format: SubtitleFormat,
    cues: &[SubtitleCue],
    translated_texts: &[String],
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = File::create(path)
        .with_context(|| format!("Không thể tạo file phụ đề: {:?}", path))?;

    if format == SubtitleFormat::Vtt {
        writeln!(file, "WEBVTT")?;
        writeln!(file)?;
    }

    for (i, cue) in cues.iter().enumerate() {
        let trans = translated_texts.get(i).map(|s| s.as_str()).unwrap_or(&cue.text);
        let start_str = format_timestamp(cue.start_ms, format);
        let end_str = format_timestamp(cue.end_ms, format);

        match format {
            SubtitleFormat::Vtt => {
                writeln!(file, "{}", i + 1)?;
                if let Some(ref settings) = cue.settings {
                    writeln!(file, "{} --> {} {}", start_str, end_str, settings)?;
                } else {
                    writeln!(file, "{} --> {}", start_str, end_str)?;
                }
                writeln!(file, "{}", trans.trim())?;
                writeln!(file)?;
            }
            SubtitleFormat::Srt => {
                writeln!(file, "{}", i + 1)?;
                writeln!(file, "{} --> {}", start_str, end_str)?;
                writeln!(file, "{}", trans.trim())?;
                writeln!(file)?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_vtt_timestamps() {
        assert_eq!(parse_timestamp("00:00:01.050"), Some(1050));
        assert_eq!(parse_timestamp("01:02:03.456"), Some(3723456));
        assert_eq!(parse_timestamp("00:00:01,050"), Some(1050));
    }
}

