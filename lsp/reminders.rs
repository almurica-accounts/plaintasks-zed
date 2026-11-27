use chrono::{DateTime, Duration as ChronoDuration, Local, NaiveDate, NaiveDateTime};
use lazy_static::lazy_static;
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

lazy_static! {
    static ref SECTION: Regex = Regex::new(r"^(\S[^:]*):\s*$").unwrap();
    static ref TASK: Regex = Regex::new(r"^\s*☐\s+(.*)$").unwrap();
    static ref DUE: Regex =
        Regex::new(r"@due\((\d{4}-\d{2}-\d{2}(?:\s+\d{2}:\d{2})?)\)").unwrap();
}

const SKIP_DIRS: &[&str] = &[
    "node_modules", ".git", ".cache", "dist", "build", ".astro", ".next",
];

#[derive(Debug, Clone)]
pub struct DueTask {
    pub key: String,
    pub title: String,
    pub where_: String,
    pub due_at: DateTime<Local>,
    pub alarm_at: DateTime<Local>,
}

fn parse_due(value: &str) -> Option<DateTime<Local>> {
    let value = value.trim();
    if let Ok(date_time) = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M") {
        return date_time.and_local_timezone(Local).single();
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return date
            .and_hms_opt(9, 0, 0)
            .and_then(|date_time| date_time.and_local_timezone(Local).single());
    }
    None
}

fn parse_board(content: &str, path: &str) -> Vec<DueTask> {
    let now = Local::now();
    let mut tasks = Vec::new();
    let mut section = String::new();
    for line in content.lines() {
        if let Some(caps) = SECTION.captures(line) {
            section = caps.get(1).unwrap().as_str().trim().to_string();
            continue;
        }
        if section.eq_ignore_ascii_case("Archive") {
            continue;
        }
        let Some(caps) = TASK.captures(line) else {
            continue;
        };
        let text = caps.get(1).unwrap().as_str();
        for found in DUE.find_iter(text) {
            let value = found
                .as_str()
                .trim_start_matches("@due(")
                .trim_end_matches(')');
            let Some(due_at) = parse_due(value) else {
                continue;
            };
            if due_at < now - ChronoDuration::days(7) {
                continue;
            }
            let title = DUE.replace_all(text, "").trim().to_string();
            let where_ = Path::new(path)
                .parent()
                .map(|dir| dir.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string());
            let key = format!("{}|{}|{}", path, value, title);
            tasks.push(DueTask {
                key,
                title,
                where_,
                alarm_at: due_at - ChronoDuration::minutes(10),
                due_at,
            });
        }
    }
    tasks
}

fn find_boards(root: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !SKIP_DIRS.contains(&name.as_str()) {
                find_boards(&path, found);
            }
        } else if entry.file_name() == "tasks.todo" {
            found.push(path);
        }
    }
}

pub fn collect(folders: &[String], open: &[(String, String)]) -> Vec<DueTask> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for folder in folders {
        find_boards(Path::new(folder), &mut paths);
    }
    for (uri, _) in open {
        if let Ok(url) = url::Url::parse(uri) {
            if let Ok(path) = url.to_file_path() {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths.dedup();

    let mut tasks = Vec::new();
    for path in paths {
        let path_string = path.to_string_lossy().to_string();
        let content = open
            .iter()
            .find(|(uri, _)| {
                url::Url::parse(uri)
                    .ok()
                    .and_then(|url| url.to_file_path().ok())
                    .map(|candidate| candidate == path)
                    .unwrap_or(false)
            })
            .map(|(_, content)| content.clone())
            .or_else(|| fs::read_to_string(&path).ok());
        if let Some(content) = content {
            tasks.extend(parse_board(&content, &path_string));
        }
    }
    tasks
}

pub fn delay_until_alarm(task: &DueTask) -> Duration {
    task.alarm_at
        .signed_duration_since(Local::now())
        .to_std()
        .unwrap_or(Duration::ZERO)
}

pub fn title(tasks: &[DueTask]) -> String {
    let overdue = tasks
        .iter()
        .filter(|task| task.due_at <= Local::now())
        .count();
    if overdue > 0 {
        format!(
            "PlainTasks: {} task(s) due now ({} overdue)",
            tasks.len(),
            overdue
        )
    } else {
        format!("PlainTasks: {} task(s) due in 10 minutes", tasks.len())
    }
}

pub fn send_notification(tasks: &[DueTask]) {
    if tasks.is_empty() {
        return;
    }
    let overdue = tasks
        .iter()
        .filter(|task| task.due_at <= Local::now())
        .count();
    let title = self::title(tasks);
    let body = tasks
        .iter()
        .map(|task| {
            let state = if task.due_at <= Local::now() {
                "DUE"
            } else {
                "IN 10 MIN"
            };
            format!(
                "{}  {} ({})\n    {}",
                state,
                task.title,
                task.due_at.format("%Y-%m-%d %H:%M"),
                task.where_
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let mut command = Command::new("notify-send");
    if std::env::var_os("DISPLAY").is_none() {
        command.env("DISPLAY", ":0");
    }
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        command.env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus");
    }
    let _ = command
        .arg(title)
        .arg(body)
        .arg("-u")
        .arg(if overdue > 0 { "critical" } else { "normal" })
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_datetime_due_and_skips_archive() {
        let past = (Local::now() - ChronoDuration::hours(1)).format("%Y-%m-%d %H:%M");
        let content = format!(
            "Inbox:\n  ☐ Call @due({})\nArchive:\n  ✔ Old @due({})\n",
            past, past
        );
        let tasks = parse_board(&content, "/tmp/project/tasks.todo");
        assert_eq!(tasks.len(), 1);
        assert_eq!(delay_until_alarm(&tasks[0]), Duration::ZERO);
        assert!(!tasks[0].key.is_empty());
    }

    #[test]
    fn schedules_ten_minutes_before_a_future_due() {
        let future = (Local::now() + ChronoDuration::hours(1)).format("%Y-%m-%d %H:%M");
        let content = format!("Inbox:\n  ☐ Later @due({})\n", future);
        let tasks = parse_board(&content, "/tmp/tasks.todo");
        assert_eq!(tasks.len(), 1);
        let delay = delay_until_alarm(&tasks[0]);
        assert!(delay > Duration::from_secs(40 * 60));
        assert!(delay < Duration::from_secs(60 * 60));
    }

    #[test]
    fn date_only_due_defaults_to_09_00() {
        let content = "Inbox:\n  ☐ Morning @due(2030-01-01)\n";
        let tasks = parse_board(content, "/tmp/tasks.todo");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].due_at.format("%H:%M").to_string(), "09:00");
    }
}
