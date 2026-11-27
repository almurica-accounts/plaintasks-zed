use chrono::{Datelike, Duration, Local, NaiveDate};
use lazy_static::lazy_static;
use regex::Regex;

/// Leading whitespace level: tabs count as one level, two spaces as one level.
pub fn indent_level(line: &str) -> usize {
    let mut level = 0usize;
    let mut spaces = 0usize;
    for ch in line.chars() {
        match ch {
            '\t' => {
                level += 1;
                spaces = 0;
            }
            ' ' => {
                spaces += 1;
                if spaces >= 2 {
                    level += 1;
                    spaces = 0;
                }
            }
            _ => break,
        }
    }
    level
}

pub fn tabs(level: usize) -> String {
    "\t".repeat(level)
}

lazy_static! {
    static ref TASK_LINE: Regex = Regex::new(r"^(\s*)([☐✔✘])\s+(.*)$").unwrap();
    static ref TAG: Regex = Regex::new(r"(?:^|\s)(@[A-Za-z][\w-]*(?:\([^)]*\))?)").unwrap();
    static ref ISO_DATE: Regex = Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap();
    static ref PHRASES: Vec<(Regex, &'static str)> = vec![
        (Regex::new(r"(?i)\bend\s+of\s+week\b").unwrap(), "end_of_week"),
        (Regex::new(r"(?i)\bend\s+of\s+month\b").unwrap(), "end_of_month"),
        (Regex::new(r"(?i)\bnext\s+week\b").unwrap(), "next_week"),
        (Regex::new(r"(?i)\bnext\s+month\b").unwrap(), "next_month"),
        (Regex::new(r"(?i)\bnext\s+year\b").unwrap(), "next_year"),
        (Regex::new(r"(?i)\bin\s+(\d+)\s+(days?|weeks?|months?|years?)\b").unwrap(), "in_units"),
        (Regex::new(r"(?i)\bnext\s+(monday|mon|tuesday|tue|tues|wednesday|wed|thursday|thu|thur|thurs|friday|fri|saturday|sat|sunday|sun)\b").unwrap(), "next_weekday"),
        (Regex::new(r"(?i)\b(?:this|on)\s+(monday|mon|tuesday|tue|tues|wednesday|wed|thursday|thu|thur|thurs|friday|fri|saturday|sat|sunday|sun)\b").unwrap(), "weekday"),
        (Regex::new(r"(?i)\btomorrow\b").unwrap(), "tomorrow"),
        (Regex::new(r"(?i)\byesterday\b").unwrap(), "yesterday"),
        (Regex::new(r"(?i)\b(?:today|tonight)\b").unwrap(), "today"),
    ];
    static ref SPACES: Regex = Regex::new(r"[ \t]{2,}").unwrap();
    static ref DONE_LINE: Regex = Regex::new(r"^\s*[✔✘]\s+").unwrap();
    static ref SECTION_LINE: Regex = Regex::new(r"^(\s*)([^:\s][^:]*):\s*$").unwrap();
    static ref DUE_TAG: Regex = Regex::new(r"\s*@due\([^)]*\)").unwrap();
}

fn add_months(date: NaiveDate, amount: i32) -> NaiveDate {
    let total = date.year() * 12 + (date.month() as i32 - 1) + amount;
    let year = total.div_euclid(12);
    let month = (total.rem_euclid(12) + 1) as u32;
    let first_next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    let last = match first_next.and_then(|d| d.pred_opt()) {
        Some(d) => d,
        None => return date,
    };
    NaiveDate::from_ymd_opt(year, month, date.day().min(last.day())).unwrap_or(last)
}

fn weekday_number(name: &str) -> Option<u32> {
    let value = name.to_lowercase();
    let number = match value.as_str() {
        "monday" | "mon" => 0,
        "tuesday" | "tue" | "tues" => 1,
        "wednesday" | "wed" => 2,
        "thursday" | "thu" | "thur" | "thurs" => 3,
        "friday" | "fri" => 4,
        "saturday" | "sat" => 5,
        "sunday" | "sun" => 6,
        _ => return None,
    };
    Some(number)
}

fn date_for(kind: &str, text: &str, base: NaiveDate) -> Option<NaiveDate> {
    match kind {
        "today" => Some(base),
        "tomorrow" => Some(base + Duration::days(1)),
        "yesterday" => Some(base - Duration::days(1)),
        "next_week" => Some(base + Duration::days(7)),
        "next_month" => Some(add_months(base, 1)),
        "next_year" => Some(add_months(base, 12)),
        "end_of_week" => {
            let delta = (4i32 - base.weekday().num_days_from_monday() as i32 + 7) % 7;
            Some(base + Duration::days(delta as i64))
        }
        "end_of_month" => {
            let first_next = if base.month() == 12 {
                NaiveDate::from_ymd_opt(base.year() + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(base.year(), base.month() + 1, 1)
            };
            first_next.and_then(|d| d.pred_opt())
        }
        "in_units" => {
            let caps = PHRASES
                .iter()
                .find(|(_, k)| *k == "in_units")
                .and_then(|(re, _)| re.captures(text))?;
            let amount: i64 = caps.get(1)?.as_str().parse().ok()?;
            let unit = caps.get(2)?.as_str().to_lowercase();
            if unit.starts_with("day") {
                Some(base + Duration::days(amount))
            } else if unit.starts_with("week") {
                Some(base + Duration::days(amount * 7))
            } else if unit.starts_with("month") {
                Some(add_months(base, amount as i32))
            } else {
                Some(add_months(base, (amount * 12) as i32))
            }
        }
        "next_weekday" | "weekday" => {
            let name = PHRASES
                .iter()
                .find(|(_, k)| *k == kind)
                .and_then(|(re, _)| re.captures(text))
                .and_then(|caps| caps.get(1).map(|m| m.as_str().to_string()))?;
            let wanted = weekday_number(&name)?;
            let current = base.weekday().num_days_from_monday();
            let mut delta = (wanted as i32 - current as i32 + 7) % 7;
            if kind == "next_weekday" && delta == 0 {
                delta = 7;
            }
            Some(base + Duration::days(delta as i64))
        }
        _ => None,
    }
}

fn resolve_phrase(text: &str, base: NaiveDate) -> Option<NaiveDate> {
    for (re, kind) in PHRASES.iter() {
        if let Some(caps) = re.captures(text) {
            if let Some(date) = date_for(kind, caps.get(0)?.as_str(), base) {
                return Some(date);
            }
        }
    }
    None
}

#[derive(Clone)]
struct Candidate {
    start: usize,
    end: usize,
    date: NaiveDate,
}

fn tag_candidate(tag: &str, base: NaiveDate) -> Option<NaiveDate> {
    let body = tag.strip_prefix('@')?;
    let (name, value) = match body.split_once('(') {
        Some((name, rest)) => (name, Some(rest.trim_end_matches(')'))),
        None => (body, None),
    };
    if name.eq_ignore_ascii_case("today") {
        return None;
    }
    if name.eq_ignore_ascii_case("due") {
        let value = value?;
        if ISO_DATE.is_match(value.trim()) {
            return None;
        }
        return resolve_phrase(value, base);
    }
    resolve_phrase(&name.replace('_', " "), base)
}

pub fn resolve_line_dates(line: &str) -> Option<String> {
    let caps = TASK_LINE.captures(line)?;
    let indent = caps.get(1)?.as_str();
    let symbol = caps.get(2)?.as_str();
    let content = caps.get(3)?.as_str();
    let base = Local::now().date_naive();

    let mut candidates: Vec<Candidate> = Vec::new();
    for found in TAG.find_iter(content) {
        let raw = found.as_str().trim_start();
        let start = found.start() + (found.as_str().len() - raw.len());
        if let Some(date) = tag_candidate(raw, base) {
            candidates.push(Candidate {
                start,
                end: start + raw.len(),
                date,
            });
        }
    }
    for (re, kind) in PHRASES.iter() {
        for found in re.find_iter(content) {
            let start = found.start();
            let end = found.end();
            if candidates
                .iter()
                .any(|c| start < c.end && end > c.start)
            {
                continue;
            }
            if let Some(date) = date_for(kind, found.as_str(), base) {
                candidates.push(Candidate { start, end, date });
            }
        }
    }
    candidates.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let mut chosen: Vec<Candidate> = Vec::new();
    for candidate in candidates {
        if chosen
            .iter()
            .any(|c| candidate.start < c.end && candidate.end > c.start)
        {
            continue;
        }
        chosen.push(candidate);
    }
    if chosen.is_empty() {
        return None;
    }

    let mut explicit: Option<String> = None;
    for found in TAG.find_iter(content) {
        let raw = found.as_str().trim_start();
        if let Some(value) = raw
            .strip_prefix("@due(")
            .and_then(|value| value.strip_suffix(')'))
        {
            if ISO_DATE.is_match(value.trim()) {
                explicit = Some(value.trim().to_string());
            }
        }
    }
    let due = explicit.unwrap_or_else(|| chosen[0].date.format("%Y-%m-%d").to_string());
    let mut updated = content.to_string();
    for (index, candidate) in chosen.iter().enumerate().rev() {
        let replacement = if index == 0 {
            String::new()
        } else {
            candidate.date.format("%Y-%m-%d").to_string()
        };
        updated.replace_range(candidate.start..candidate.end, &replacement);
    }
    updated = DUE_TAG.replace_all(&updated, "").trim().to_string();
    updated = SPACES.replace_all(&updated, " ").trim().to_string();
    updated = format!("{} @due({})", updated, due);
    Some(format!("{}{} {}", indent, symbol, updated))
}

fn is_section(line: &str) -> bool {
    SECTION_LINE.is_match(line)
}

fn indent_of(line: &str) -> usize {
    line.chars().take_while(|c| c.is_whitespace()).count()
}

pub fn archive_completed(content: &str) -> Option<String> {
    let eol = if content.contains("\r\n") {
        "\r\n"
    } else if content.contains('\r') {
        "\r"
    } else {
        "\n"
    };
    let mut lines: Vec<String> = content.lines().map(|line| line.to_string()).collect();
    while lines.last().map_or(false, |line| line.is_empty()) {
        lines.pop();
    }

    let archive_index = lines.iter().position(|line| {
        indent_of(line) == 0
            && line.trim_end().eq_ignore_ascii_case("Archive:")
    });

    let boundaries: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| is_section(line) && indent_of(line) == 0)
        .map(|(index, _)| index)
        .collect();

    let archive_end = archive_index.map(|start| {
        boundaries
            .iter()
            .find(|index| **index > start)
            .copied()
            .unwrap_or_else(|| lines.len())
    });

    let mut candidates: Vec<(usize, usize)> = Vec::new();
    let mut index = 0usize;
    while index < lines.len() {
        let in_archive = match archive_index {
            Some(start) => index >= start && index < archive_end.unwrap_or(lines.len()),
            None => false,
        };
        if !in_archive && DONE_LINE.is_match(&lines[index]) {
            let task_indent = indent_of(&lines[index]);
            let mut end = index + 1;
            while end < lines.len() {
                let line = &lines[end];
                let is_section = is_section(line) && indent_of(line) <= task_indent;
                let is_task = TASK_LINE.is_match(line) && indent_of(line) <= task_indent;
                if is_section || is_task {
                    break;
                }
                end += 1;
            }
            candidates.push((index, end));
            index = end;
        } else {
            index += 1;
        }
    }
    if candidates.is_empty() {
        return None;
    }

    let mut blocks: Vec<Vec<String>> = Vec::new();
    for &(start, end) in candidates.iter().rev() {
        blocks.push(lines[start..end].to_vec());
        lines.drain(start..end);
    }
    blocks.reverse();

    let adjusted: Vec<String> = blocks
        .into_iter()
        .flatten()
        .map(|line| {
            if TASK_LINE.is_match(&line) {
                let caps = TASK_LINE.captures(&line).unwrap();
                format!(
                    "\t{} {}",
                    caps.get(2).unwrap().as_str(),
                    caps.get(3).unwrap().as_str()
                )
            } else {
                line
            }
        })
        .collect();

    let archive_start = lines
        .iter()
        .position(|line| indent_of(line) == 0 && line.trim_end().eq_ignore_ascii_case("Archive:"));
    let insert_at = match archive_start {
        Some(start) => {
            let mut at = start + 1;
            while at < lines.len() && lines[at].trim().is_empty() {
                at += 1;
            }
            at
        }
        None => {
            if !lines.is_empty() && !lines.last().unwrap().trim().is_empty() {
                lines.push(String::new());
            }
            lines.push("Archive:".to_string());
            lines.len()
        }
    };
    lines.splice(insert_at..insert_at, adjusted);
    lines.push(String::new());
    Some(lines.join(eol))
}


/// Resolve date keywords and tags on every task line in the document.
pub fn resolve_file_dates(content: &str) -> Option<String> {
    let eol = if content.contains("\r\n") {
        "\r\n"
    } else if content.contains('\r') {
        "\r"
    } else {
        "\n"
    };
    let mut lines: Vec<String> = content.lines().map(|line| line.to_string()).collect();
    let mut changed = false;
    for line in lines.iter_mut() {
        if let Some(resolved) = resolve_line_dates(line) {
            *line = resolved;
            changed = true;
        }
    }
    if !changed {
        return None;
    }
    while lines.last().map_or(false, |line| line.is_empty()) {
        lines.pop();
    }
    lines.push(String::new());
    Some(lines.join(eol))
}

/// Resolve only date-like tags on a task line (used for automatic resolution
/// while typing). Plain-language date phrases are left alone.
pub fn resolve_date_tags(line: &str) -> Option<String> {
    let caps = TASK_LINE.captures(line)?;
    let indent = caps.get(1)?.as_str();
    let symbol = caps.get(2)?.as_str();
    let content = caps.get(3)?.as_str();
    let base = Local::now().date_naive();

    let mut candidates: Vec<Candidate> = Vec::new();
    for found in TAG.find_iter(content) {
        let raw = found.as_str().trim_start();
        let start = found.start() + (found.as_str().len() - raw.len());
        if let Some(date) = tag_candidate(raw, base) {
            candidates.push(Candidate {
                start,
                end: start + raw.len(),
                date,
            });
        }
    }
    if candidates.is_empty() {
        return None;
    }

    let mut explicit: Option<String> = None;
    for found in TAG.find_iter(content) {
        let raw = found.as_str().trim_start();
        if let Some(value) = raw
            .strip_prefix("@due(")
            .and_then(|value| value.strip_suffix(')'))
        {
            if ISO_DATE.is_match(value.trim()) {
                explicit = Some(value.trim().to_string());
            }
        }
    }
    let due = explicit.unwrap_or_else(|| candidates[0].date.format("%Y-%m-%d").to_string());

    let mut updated = content.to_string();
    for (index, candidate) in candidates.iter().enumerate().rev() {
        let replacement = if index == 0 {
            String::new()
        } else {
            candidate.date.format("%Y-%m-%d").to_string()
        };
        updated.replace_range(candidate.start..candidate.end, &replacement);
    }
    updated = DUE_TAG.replace_all(&updated, "").trim().to_string();
    updated = SPACES.replace_all(&updated, " ").trim().to_string();
    Some(format!("{}{} {} @due({})", indent, symbol, updated, due))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_tomorrow_tag() {
        let resolved = resolve_line_dates("  ☐ Call client @tomorrow @high");
        let value = resolved.unwrap();
        assert!(value.contains("@high"));
        assert!(value.contains("@due("));
        assert!(!value.contains("@tomorrow"));
    }

    #[test]
    fn leaves_lines_without_dates_alone() {
        assert!(resolve_line_dates("  ☐ Call client @high").is_none());
    }

    #[test]
    fn resolves_text_phrases_and_tags() {
        let value = resolve_line_dates("  ☐ Call client tomorrow").unwrap();
        assert!(value.contains("@due("));
        assert!(!value.contains("tomorrow"));
        let value = resolve_line_dates("  ☐ Call client @next_week").unwrap();
        assert!(value.contains("@due("));
        assert!(!value.contains("@next_week"));
    }

    #[test]
    fn explicit_due_wins() {
        let value = resolve_line_dates("  ☐ Call client tomorrow @due(2026-12-01)").unwrap();
        assert!(value.contains("@due(2026-12-01)"));
        assert!(!value.contains("tomorrow"));
    }

    #[test]
    fn archives_done_tasks() {
        let input = "Inbox:\n  ☐ Open\n  ✔ Done @done(26-11-26 10:00)\nArchive:\n";
        let output = archive_completed(input).unwrap();
        assert!(output.contains("Inbox:\n  ☐ Open\nArchive:\n\t✔ Done"));
    }

    #[test]
    fn auto_resolves_date_tags_only() {
        assert!(resolve_date_tags("  ☐ Call tomorrow").is_none());
        assert!(resolve_date_tags("  ☐ Call @high @today").is_none());
        let value = resolve_date_tags("  ☐ Call @tomorrow @high").unwrap();
        assert!(value.contains("@high @due("));
        assert!(!value.contains("@tomorrow"));
        let value = resolve_date_tags("  ☐ Review @end_of_month").unwrap();
        assert!(value.contains("@due("));
        assert!(!value.contains("@end_of_month"));
    }

    #[test]
    fn resolves_dates_across_the_whole_file() {
        let input = "Inbox:\n  ☐ A tomorrow\n  ☐ B @next_week\nArchive:\n";
        let output = resolve_file_dates(input).unwrap();
        assert!(output.contains("A @due("));
        assert!(output.contains("B @due("));
        assert!(!output.contains("tomorrow"));
        assert!(!output.contains("@next_week"));
    }

    #[test]
    fn indentation_uses_tabs() {
        assert_eq!(indent_level("  ☐ task"), 1);
        assert_eq!(indent_level("\t☐ task"), 1);
        assert_eq!(indent_level("    ☐ task"), 2);
        assert_eq!(tabs(2), "\t\t");
    }
}
