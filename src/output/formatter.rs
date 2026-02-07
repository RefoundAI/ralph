//! Terminal output formatting with ANSI colors.

use colored::Colorize;
use std::collections::HashMap;
use std::process::Command;

use crate::claude::events::{ContentBlock, Event};
use crate::config::Config;

/// Info about a tool call for error reporting.
#[derive(Clone)]
pub struct ToolCallInfo {
    pub name: String,
    pub input: HashMap<String, serde_json::Value>,
}

/// Format and print an event to the terminal.
pub fn format_event(event: &Event, tool_calls: &mut HashMap<String, ToolCallInfo>) {
    format_event_inner(event, tool_calls, false);
}

/// Format and print an event, suppressing text content blocks.
///
/// Shows thinking and result events but hides text output.
/// Used by the plan command where the text IS the JSON blob.
pub fn format_event_no_text(event: &Event, tool_calls: &mut HashMap<String, ToolCallInfo>) {
    format_event_inner(event, tool_calls, true);
}

fn format_event_inner(
    event: &Event,
    tool_calls: &mut HashMap<String, ToolCallInfo>,
    suppress_text: bool,
) {
    match event {
        Event::Assistant(assistant) => {
            if !suppress_text {
                let has_text = assistant
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Text { .. }));
                if has_text {
                    if let Some(model) = &assistant.model {
                        println!("{}", format!("→ {}", model).purple());
                    }
                }
            }
            for block in &assistant.content {
                match block {
                    ContentBlock::Text { .. } if suppress_text => {}
                    _ => format_content_block(block, tool_calls),
                }
            }
        }
        Event::ToolErrors(errors) => {
            for error in errors {
                let tool_info = tool_calls.get(&error.tool_use_id);
                let tool_name = tool_info
                    .map(|t| t.name.as_str())
                    .unwrap_or("unknown");

                println!("{}", format!("✗ {} failed", tool_name).red());

                if let Some(info) = tool_info {
                    if !info.input.is_empty() {
                        println!("  {}", format_input(&info.input).dimmed());
                    }
                }

                // Show first 5 lines of error, stripping XML tags
                for line in error.content.lines().take(5) {
                    let clean = strip_xml_tags(line);
                    if !clean.is_empty() {
                        println!("  {}", clean.red());
                    }
                }
            }
        }
        Event::StreamDelta(delta) => {
            // Stream deltas are always shown (real-time feedback).
            // suppress_text only applies to the final complete assistant message.
            use std::io::Write;
            match delta.delta_type.as_str() {
                "thinking_delta" => {
                    print!("{}", delta.text.bright_black());
                    std::io::stdout().flush().ok();
                }
                "text_delta" => {
                    print!("{}", delta.text.bright_white());
                    std::io::stdout().flush().ok();
                }
                _ => {}
            }
        }
        Event::Result(result) => {
            let duration_s = result.duration_ms / 1000;
            let cost = format!("{:.2}", result.total_cost_usd);

            println!();
            println!("{}", format!("✓ Done ({}s, ${})", duration_s, cost).green());
        }
        Event::Unknown => {}
    }
}

fn format_content_block(block: &ContentBlock, tool_calls: &mut HashMap<String, ToolCallInfo>) {
    match block {
        ContentBlock::Text { text } => {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                for line in trimmed.lines() {
                    println!("{}", line.bright_white());
                }
            }
        }
        ContentBlock::Thinking { thinking } => {
            for line in thinking.lines() {
                println!("{} {}", "┊".bright_black(), line.bright_black());
            }
        }
        ContentBlock::ToolUse { id, name, input } => {
            println!(
                "{} {}",
                format!("→ {}", name).cyan(),
                format_input(input).dimmed()
            );
            tool_calls.insert(
                id.clone(),
                ToolCallInfo {
                    name: name.clone(),
                    input: input.clone(),
                },
            );
        }
        ContentBlock::Unknown => {}
    }
}

fn strip_xml_tags(s: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    result.trim().to_string()
}

fn format_input(input: &HashMap<String, serde_json::Value>) -> String {
    input
        .iter()
        .map(|(k, v)| {
            let v_str = v.to_string();
            let v_truncated = if v_str.len() > 80 {
                format!("{}...", &v_str[..77])
            } else {
                v_str
            };
            format!("{}={}", k, v_truncated)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Print iteration information.
pub fn print_iteration_info(config: &Config) {
    if config.limit == 1 {
        println!("Running once only");
    } else if config.total == 0 {
        println!("Iteration {} (unlimited)", config.iteration);
    } else {
        println!("Iteration {} of {}", config.iteration, config.total);
    }
}

/// Print sandbox modification info.
#[allow(dead_code)]
pub fn print_sandbox_mods(allow_rules: &[String], readonly_dirs: &[String], writeable_dirs: &[String]) {
    if !allow_rules.is_empty() {
        println!("{} {}", "+allow:".green(), allow_rules.join(" "));

        if !readonly_dirs.is_empty() {
            println!("{} {}", "+sandbox read:".green(), readonly_dirs.join(" "));
        }

        if !writeable_dirs.is_empty() {
            println!("{} {}", "+sandbox write:".green(), writeable_dirs.join(" "));
        }
    }
}

/// Print sandbox warning.
pub fn print_sandbox_warning() {
    println!(
        "{}",
        "Warning: Using --dangerously-skip-permissions with sandbox-exec. Use --no-sandbox for safer permissions.".yellow()
    );
}

/// Print completion message.
pub fn print_complete() {
    println!("Tasks complete.");
    speak("Ralph finished. Tasks complete.");
}

/// Print failure message.
pub fn print_failure() {
    println!("Critical failure. See progress file for details.");
    speak("Ralph failed--critical failure.");
}

/// Print limit reached message.
pub fn print_limit_reached() {
    speak("Ralph finished--limit hit.");
}

/// Print iteration separator.
pub fn print_separator() {
    let width = terminal_width();
    println!("{}", "-".repeat(width).dimmed());
}

/// Print a clickable file hyperlink.
pub fn hyperlink(path: &str) {
    println!("\x1b]8;;file://{}\x1b\\{}\x1b]8;;\x1b\\", path, path);
}

fn speak(message: &str) {
    if Command::new("which").arg("say").output().is_ok() {
        let msg = message.to_string();
        std::thread::spawn(move || {
            let _ = Command::new("say").arg(&msg).output();
        });
    }
}

fn terminal_width() -> usize {
    Command::new("tput")
        .arg("cols")
        .output()
        .ok()
        .and_then(|out| {
            if out.status.success() {
                String::from_utf8_lossy(&out.stdout)
                    .trim()
                    .parse()
                    .ok()
            } else {
                None
            }
        })
        .unwrap_or(80)
}
