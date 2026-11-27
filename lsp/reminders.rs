use chrono::Local;
use lazy_static::lazy_static;
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

lazy_static! {
    static ref SECTION: Regex = Regex::new(r"^(\S[^:]*):\s*$").unwrap();
    static ref TASK: Regex = Regex::new(r"^\s*☐\s+(.*)$").unwrap();
    static ref DUE: Regex = Regex::new(r"@due\((\d{4}-\d{2}-\d{2})\)").unwrap();
}

const SKIP_DIRS: &[&str] = &[
    "node_modules", ".git", ".cache", "dist", "build", ".astro", ".next",
];

#[derive(Debug, Clone)]
pub struct DueTask {
    pub key: String,
    pub title: String,
    pub where_: String,
    pub date: String,
    pub overdue: bool,
}

fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

fn parse_board(content: &str, path: &str) -> Vec<DueTask> {
    let current = today();
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
            let date = found
                .as_str()
                .trim_start_matches("@due(")
                .trim_end_matches(')')
                .to_string();
            if date > current {
                continue;
            }
            let title = DUE.replace_all(text, "").trim().to_string();
            let where_ = Path::new(path)
                .parent()
                .map(|dir| dir.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string());
            let key = format!("{}|{}|{}", path, date, title);
            let overdue = date < current;
            tasks.push(DueTask {
                key,
                title: title.clone(),
                where_,
                date,
                overdue,
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

pub fn send_notification(tasks: &[DueTask]) {
    if tasks.is_empty() {
        return;
    }
    let overdue = tasks.iter().filter(|task| task.overdue).count();
    let title = if overdue > 0 {
        format!(
            "PlainTasks: {} task(s) need attention ({} overdue)",
            tasks.len(),
            overdue
        )
    } else {
        format!("PlainTasks: {} task(s) due today", tasks.len())
    };
    let body = tasks
        .iter()
        .map(|task| {
            format!(
                "{}  {}\n    {}",
                if task.overdue { "OVERDUE" } else { "TODAY" },
                task.title,
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
    fn collects_due_tasks_and_skips_archive() {
        let content = "Inbox:\n  ☐ Call tomorrow @due(2020-01-01)\nArchive:\n  ✔ Old @due(2020-01-01)\n";
        let tasks = parse_board(content, "/tmp/project/tasks.todo");
        assert_eq!(tasks.len(), 1);
        assert!(tasks[0].overdue);
        assert!(!tasks[0].title.contains("@due"));
    }

    #[test]
    fn ignores_future_dates() {
        let future = Local::now().date_naive() + chrono::Duration::days(2);
        let content = format!(
            "Inbox:\n  ☐ Later @due({})\n",
            future.format("%Y-%m-%d")
        );
        assert!(parse_board(&content, "/tmp/tasks.todo").is_empty());
    }
}
