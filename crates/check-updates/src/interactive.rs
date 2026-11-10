use std::future::Future;
use std::sync::{Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use console::{Key, Term};
use owo_colors::{OwoColorize, Stream, Style};

static SPINNER_LINE: Mutex<Option<String>> = Mutex::new(None);

/// Write logs without overwriting the spinner.
pub struct Logger;

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let level = match record.level() {
            log::Level::Error => Style::new().red(),
            log::Level::Warn => Style::new().yellow(),
            log::Level::Info => Style::new().blue(),
            log::Level::Debug => Style::new().cyan(),
            log::Level::Trace => Style::new().magenta(),
        };
        let message = format!(
            "{}: {}",
            record
                .level()
                .if_supports_color(Stream::Stderr, |text| text.style(level)),
            display_text(&record.args().to_string())
        );
        let line = SPINNER_LINE.lock().unwrap();
        let term = Term::stderr();
        if line.is_some() {
            let _ = term.clear_line();
        }
        let _ = term.write_line(&message);
        if let Some(line) = line.as_ref() {
            let _ = term.write_str(line);
        }
    }

    fn flush(&self) {
        let _ = Term::stderr().flush();
    }
}

struct Spinner {
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Spinner {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Show a spinner while a future runs.
pub async fn with_spinner<F: Future>(message: &str, future: F) -> F::Output {
    let term = Term::stderr();
    if !term.is_term() {
        return future.await;
    }

    let message = message.to_owned();
    let (stop, receiver) = mpsc::channel();
    let Ok(thread) = thread::Builder::new().spawn(move || {
        for frame in "⠁⠁⠉⠙⠚⠒⠂⠂⠒⠲⠴⠤⠄⠄⠤⠠⠠⠤⠦⠖⠒⠐⠐⠒⠓⠋⠉⠈⠈".chars().cycle()
        {
            {
                let line = format!(
                    "{} {message}",
                    frame.if_supports_color(Stream::Stderr, |frame| frame.cyan())
                );
                let width = term.size().1.saturating_sub(1) as usize;
                let line = console::truncate_str(&line, width, "").into_owned();
                let mut spinner_line = SPINNER_LINE.lock().unwrap();
                let _ = term.clear_line();
                let _ = term.write_str(&line);
                *spinner_line = Some(line);
            }
            match receiver.recv_timeout(Duration::from_millis(80)) {
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                _ => break,
            }
        }
        let mut line = SPINNER_LINE.lock().unwrap();
        if line.take().is_some() {
            let _ = term.clear_line();
        }
    }) else {
        return future.await;
    };
    let _spinner = Spinner {
        stop,
        thread: Some(thread),
    };
    future.await
}

/// Display-ready updates from any backend.
#[derive(Clone)]
pub struct UpdateGroup {
    pub name: String,
    pub updates: Vec<String>,
}

pub fn display_text(text: &str) -> String {
    let mut safe = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control() {
            safe.extend(character.escape_default());
        } else {
            safe.push(character);
        }
    }
    safe
}

pub fn print_groups(groups: &[UpdateGroup], show_headers: bool) {
    for (index, group) in groups.iter().enumerate() {
        if index > 0 {
            println!();
        }
        if show_headers {
            println!(
                "{}",
                group
                    .name
                    .if_supports_color(Stream::Stdout, |text| text.bold())
            );
        }
        for update in &group.updates {
            println!("{update}");
        }
    }
}

/// Interactive inline picker for dependency updates.
pub fn prompt_updates(
    groups: &[UpdateGroup],
    default_selected: &[bool],
    compact: bool,
) -> std::io::Result<Vec<usize>> {
    if groups.is_empty() {
        return Ok(Vec::new());
    }

    let term = Term::stderr();
    let (height, width) = term.size();

    let compact = compact || height < 15;
    let min_height = if compact { 5 } else { 7 };

    if height < min_height {
        return Err(std::io::Error::other(format!(
            "Terminal height too small (need at least {min_height} lines)"
        )));
    }
    if width < 20 {
        return Err(std::io::Error::other(
            "Terminal width too small (need at least 20 columns)",
        ));
    }

    InlineSelect::new(&term, groups, default_selected, compact)?.run()
}

enum LineKind {
    Group(String),
    Separator,
    Update { cursor_idx: usize, label: String },
}

struct InlineSelect<'t> {
    term: &'t Term,
    lines: Vec<LineKind>,
    selected: Vec<bool>,
    update_line_for_cursor: Vec<usize>,
    group_line_for_cursor: Vec<usize>,
    cursor: usize,
    scroll_offset: usize,
    header_lines: usize,
    window_lines: usize,
    compact: bool,
    colors: bool,
}

impl Drop for InlineSelect<'_> {
    fn drop(&mut self) {
        let _ = self.term.show_cursor();
    }
}

impl<'t> InlineSelect<'t> {
    fn new(
        term: &'t Term,
        groups: &[UpdateGroup],
        default_selected: &[bool],
        compact: bool,
    ) -> std::io::Result<Self> {
        let mut lines = Vec::new();
        let mut update_line_for_cursor = Vec::new();
        let mut group_line_for_cursor = Vec::new();
        let mut cursor_idx = 0usize;

        for (group_idx, group) in groups.iter().enumerate() {
            if !compact && group_idx > 0 {
                lines.push(LineKind::Separator);
            }
            let group_line_idx = lines.len();
            lines.push(LineKind::Group(group.name.clone()));

            for label in &group.updates {
                update_line_for_cursor.push(lines.len());
                group_line_for_cursor.push(group_line_idx);
                lines.push(LineKind::Update {
                    cursor_idx,
                    label: label.clone(),
                });
                cursor_idx += 1;
            }
        }

        assert_eq!(default_selected.len(), cursor_idx);

        let term_height = term.size().0 as usize;
        let header_lines = if compact { 1 } else { 3 };
        let window_lines = term_height.saturating_sub(header_lines).max(1);

        let select = Self {
            term,
            lines,
            selected: default_selected.to_vec(),
            update_line_for_cursor,
            group_line_for_cursor,
            cursor: 0,
            scroll_offset: 0,
            header_lines,
            window_lines,
            compact,
            colors: console::colors_enabled_stderr(),
        };

        term.hide_cursor()?;
        select.render_content()?;
        Ok(select)
    }

    fn run(mut self) -> std::io::Result<Vec<usize>> {
        loop {
            let key = match self.term.read_key() {
                Ok(key) => key,
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {
                    self.clear()?;
                    std::process::exit(130);
                }
                Err(err) => return Err(err),
            };

            match key {
                Key::ArrowUp | Key::Char('k') => self.move_cursor(-1)?,
                Key::ArrowDown | Key::Char('j') => self.move_cursor(1)?,
                Key::Char(' ') => self.toggle_and_advance()?,
                Key::Enter => {
                    self.clear()?;
                    return Ok(self.collect_selected());
                }
                Key::Escape | Key::Char('q') => {
                    self.clear()?;
                    std::process::exit(0);
                }
                Key::CtrlC => {
                    self.clear()?;
                    std::process::exit(130);
                }
                _ => {}
            }
        }
    }

    fn total_lines(&self) -> usize {
        self.header_lines
            + if self.needs_scroll() {
                self.window_lines
            } else {
                self.lines.len()
            }
    }

    fn needs_scroll(&self) -> bool {
        self.lines.len() > self.window_lines
    }

    fn skip_separators(&self, mut start: usize) -> usize {
        while start < self.lines.len() && matches!(self.lines[start], LineKind::Separator) {
            start += 1;
        }
        start
    }

    fn move_cursor(&mut self, delta: isize) -> std::io::Result<()> {
        let count = self.update_line_for_cursor.len();
        self.cursor = (self.cursor as isize + delta).rem_euclid(count as isize) as usize;
        self.adjust_scroll();
        self.render()
    }

    fn toggle_and_advance(&mut self) -> std::io::Result<()> {
        self.selected[self.cursor] = !self.selected[self.cursor];
        if self.update_line_for_cursor.len() > 1 {
            self.cursor = (self.cursor + 1) % self.update_line_for_cursor.len();
        }
        self.adjust_scroll();
        self.render()
    }

    fn adjust_scroll(&mut self) {
        if !self.needs_scroll() {
            self.scroll_offset = 0;
            return;
        }

        let cursor_line = self.update_line_for_cursor[self.cursor];
        let group_line = self.group_line_for_cursor[self.cursor];

        if cursor_line < self.scroll_offset {
            self.scroll_offset = cursor_line;
        } else if cursor_line >= self.scroll_offset + self.window_lines {
            self.scroll_offset = cursor_line - self.window_lines + 1;
        }

        if self.cursor_is_first_in_group()
            && (group_line < self.scroll_offset
                || group_line >= self.scroll_offset + self.window_lines)
        {
            // When entering a new group, snap to its header first.
            self.scroll_offset = group_line;
        }

        self.scroll_offset = self.skip_separators(self.scroll_offset);

        // once the group header scrolls past the top,
        // reserve one row for that header so the current item still remains visible.
        if self.group_is_sticky(self.scroll_offset) {
            let visible = self.window_lines.saturating_sub(1).max(1);
            if cursor_line >= self.scroll_offset + visible {
                self.scroll_offset = cursor_line - visible + 1;
            }
            self.scroll_offset = self.skip_separators(self.scroll_offset);
        }
    }

    fn cursor_is_first_in_group(&self) -> bool {
        let update_line = self.update_line_for_cursor[self.cursor];
        update_line > 0 && matches!(self.lines[update_line - 1], LineKind::Group(_))
    }

    fn group_is_sticky(&self, start: usize) -> bool {
        self.group_line_for_cursor[self.cursor] < start
    }

    fn collect_selected(self) -> Vec<usize> {
        self.selected
            .iter()
            .enumerate()
            .filter_map(|(index, selected)| selected.then_some(index))
            .collect()
    }

    fn render(&self) -> std::io::Result<()> {
        self.term
            .move_cursor_up(self.total_lines().saturating_sub(1))?;
        self.term.clear_to_end_of_screen()?;
        self.render_content()
    }

    fn render_content(&self) -> std::io::Result<()> {
        let mut frame = Vec::with_capacity(self.total_lines());

        if !self.compact {
            frame.push(String::new());
        }

        frame.push(format!(
            "  {}",
            "Choose which packages to update".if_supports_color(Stream::Stderr, |text| text.bold())
        ));

        if !self.compact {
            let nav = if self.needs_scroll() {
                format!(
                    "  {} / {}  (↑↓ jk navigate, space toggle, enter confirm)",
                    self.cursor + 1,
                    self.update_line_for_cursor.len()
                )
            } else {
                "  (↑↓ jk navigate, space toggle, enter confirm)".to_string()
            };
            frame.push(
                nav.if_supports_color(Stream::Stderr, |text| text.dimmed())
                    .to_string(),
            );
        }

        let mut written = 0;
        let start = if self.needs_scroll() {
            self.scroll_offset
        } else {
            0
        };
        let target = if self.needs_scroll() {
            self.window_lines
        } else {
            self.lines.len()
        };
        let sticky_group = self.group_is_sticky(start);
        if sticky_group {
            let group_line = self.group_line_for_cursor[self.cursor];
            if let LineKind::Group(name) = &self.lines[group_line] {
                frame.push(format!(
                    "  {}",
                    name.if_supports_color(Stream::Stderr, |text| text
                        .style(Style::new().blue().bold()))
                ));
                written += 1;
            }
        }
        let end = (start + target).min(self.lines.len());

        for idx in start..end {
            match &self.lines[idx] {
                LineKind::Separator => {
                    if !self.compact {
                        frame.push(String::new());
                        written += 1;
                    }
                }
                LineKind::Group(name) => {
                    if !sticky_group || idx != self.group_line_for_cursor[self.cursor] {
                        frame.push(format!(
                            "  {}",
                            name.if_supports_color(Stream::Stderr, |text| text
                                .style(Style::new().blue().bold()))
                        ));
                        written += 1;
                    }
                }
                LineKind::Update {
                    cursor_idx, label, ..
                } => {
                    let is_current = *cursor_idx == self.cursor;
                    let checkbox = self.format_checkbox(self.selected[*cursor_idx], is_current);
                    frame.push(if is_current {
                        format!(
                            "{} {checkbox} {label}",
                            ">".if_supports_color(Stream::Stderr, |text| text
                                .style(Style::new().cyan().bold()))
                        )
                    } else {
                        format!(
                            "  {checkbox} {}",
                            label.if_supports_color(Stream::Stderr, |text| text.dimmed())
                        )
                    });
                    written += 1;
                }
            }

            if written >= target {
                break;
            }
        }

        for _ in written..target {
            frame.push(String::new());
        }

        // Keep the cursor anchored in-place by avoiding an extra trailing newline
        for line in frame.iter().take(frame.len() - 1) {
            self.term.write_line(line)?;
        }
        self.term.write_str(frame.last().unwrap())?;

        Ok(())
    }

    fn format_checkbox(&self, selected: bool, current: bool) -> String {
        let sym = if self.colors {
            if selected { "◉" } else { "◯" }
        } else if selected {
            "[x]"
        } else {
            "[ ]"
        };

        let mut style = Style::new();
        if selected && self.colors {
            style = style.green();
        }
        if current {
            style = style.bold();
            if self.colors {
                style = style.cyan();
            }
        }
        sym.if_supports_color(Stream::Stderr, |text| text.style(style))
            .to_string()
    }

    fn clear(&self) -> std::io::Result<()> {
        self.term.show_cursor()?;
        self.term
            .move_cursor_up(self.total_lines().saturating_sub(1))?;
        self.term.clear_to_end_of_screen()
    }
}
