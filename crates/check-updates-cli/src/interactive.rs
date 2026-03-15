use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use check_updates::{Package, Unit, Usage};
use console::{Key, Term, style};
use semver::VersionReq;

use crate::update::{Update, format_update_line};

/// Interactive inline picker for dependency updates.
pub fn prompt_updates<'a>(
    updates: &BTreeMap<&'a Unit, Vec<Update<'a>>>,
    compact: bool,
) -> std::io::Result<Vec<(&'a Usage, &'a Package, VersionReq)>> {
    if updates.is_empty() {
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

    InlineSelect::new(&term, updates, compact)?.run()
}

enum LineKind<'a> {
    Group(String),
    Separator,
    Update {
        cursor_idx: usize,
        label: String,
        usage: &'a Usage,
        package: &'a Package,
        new_req: VersionReq,
    },
}

struct InlineSelect<'a, 't> {
    term: &'t Term,
    lines: Vec<LineKind<'a>>,
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

struct CtrlCGuard {
    active: Arc<Mutex<bool>>,
}

impl Drop for CtrlCGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = self.active.lock() {
            *active = false;
        }
    }
}

impl<'a, 't> InlineSelect<'a, 't> {
    fn new(
        term: &'t Term,
        updates: &BTreeMap<&'a Unit, Vec<Update<'a>>>,
        compact: bool,
    ) -> std::io::Result<Self> {
        let (name_w, cur_w, new_w) = global_column_widths(updates, term);

        let mut lines = Vec::new();
        let mut selected = Vec::new();
        let mut update_line_for_cursor = Vec::new();
        let mut group_line_for_cursor = Vec::new();
        let mut cursor_idx = 0usize;

        for (unit_idx, (unit, unit_updates)) in updates.iter().enumerate() {
            if !compact && unit_idx > 0 {
                lines.push(LineKind::Separator);
            }
            let group_line_idx = lines.len();
            lines.push(LineKind::Group(unit.name().to_string()));

            for update in unit_updates {
                update_line_for_cursor.push(lines.len());
                group_line_for_cursor.push(group_line_idx);
                selected.push(true);
                lines.push(LineKind::Update {
                    cursor_idx,
                    label: format_update_line(update, name_w, cur_w, new_w),
                    usage: update.usage,
                    package: update.package,
                    new_req: update.new_req.clone(),
                });
                cursor_idx += 1;
            }
        }

        let term_height = term.size().0 as usize;
        let header_lines = if compact { 1 } else { 3 };
        let window_lines = term_height.saturating_sub(header_lines).max(1);

        let select = Self {
            term,
            lines,
            selected,
            update_line_for_cursor,
            group_line_for_cursor,
            cursor: 0,
            scroll_offset: 0,
            header_lines,
            window_lines,
            compact,
            colors: term.features().colors_supported(),
        };

        select.reserve_space_and_render()?;
        Ok(select)
    }

    fn run(mut self) -> std::io::Result<Vec<(&'a Usage, &'a Package, VersionReq)>> {
        let ctrlc_guard = self.install_ctrlc_handler();
        loop {
            let key = match self.term.read_key() {
                Ok(key) => key,
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {
                    drop(ctrlc_guard);
                    self.clear()?;
                    std::process::exit(130);
                }
                Err(err) => return Err(err),
            };

            match key {
                Key::ArrowUp | Key::Char('k') => self.move_cursor(-1),
                Key::ArrowDown | Key::Char('j') => self.move_cursor(1),
                Key::Char(' ') => self.toggle_and_advance(),
                Key::Enter => {
                    drop(ctrlc_guard);
                    self.clear()?;
                    return Ok(self.collect_selected());
                }
                Key::Escape | Key::Char('q') => {
                    drop(ctrlc_guard);
                    self.clear()?;
                    std::process::exit(0);
                }
                _ => {}
            }
        }
    }

    fn install_ctrlc_handler(&self) -> Option<CtrlCGuard> {
        let term = self.term.clone();
        let lines = self.total_lines();
        let active = Arc::new(Mutex::new(true));
        let active_for_handler = Arc::clone(&active);

        ctrlc::set_handler(move || {
            let should_clear = active_for_handler
                .lock()
                .map(|active| *active)
                .unwrap_or(false);
            if should_clear {
                let _ = term.show_cursor();
                let _ = term.move_cursor_up(lines.saturating_sub(1));
                let _ = term.clear_to_end_of_screen();
            }
            std::process::exit(130);
        })
        .ok()
        .map(|_| CtrlCGuard { active })
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

    fn move_cursor(&mut self, delta: isize) {
        let count = self.update_line_for_cursor.len();
        self.cursor = (self.cursor as isize + delta).rem_euclid(count as isize) as usize;
        self.adjust_scroll();
        let _ = self.render();
    }

    fn toggle_and_advance(&mut self) {
        self.selected[self.cursor] = !self.selected[self.cursor];
        if self.update_line_for_cursor.len() > 1 {
            self.cursor = (self.cursor + 1) % self.update_line_for_cursor.len();
        }
        self.adjust_scroll();
        let _ = self.render();
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

    fn collect_selected(self) -> Vec<(&'a Usage, &'a Package, VersionReq)> {
        self.lines
            .into_iter()
            .filter_map(|line| match line {
                LineKind::Update {
                    cursor_idx,
                    usage,
                    package,
                    new_req,
                    ..
                } if self.selected[cursor_idx] => Some((usage, package, new_req)),
                _ => None,
            })
            .collect()
    }

    fn reserve_space_and_render(&self) -> std::io::Result<()> {
        self.term.hide_cursor()?;
        self.render_content()
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
            style("Choose which packages to update").bold()
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
            frame.push(style(nav).dim().to_string());
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
                frame.push(format!("  {}", style(name).blue().bold()));
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
                        frame.push(format!("  {}", style(name).blue().bold()));
                        written += 1;
                    }
                }
                LineKind::Update {
                    cursor_idx, label, ..
                } => {
                    let is_current = *cursor_idx == self.cursor;
                    let checkbox = self.format_checkbox(self.selected[*cursor_idx], is_current);
                    let prefix = if is_current { ">" } else { " " };

                    let line = if is_current {
                        format!("{} {} {}", style(prefix).cyan().bold(), checkbox, label)
                    } else {
                        format!("{} {} {}", prefix, checkbox, style(label).dim())
                    };
                    frame.push(line);
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

        if frame.is_empty() {
            return Ok(());
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

        let styled = if selected && self.colors {
            style(sym).green().to_string()
        } else {
            sym.to_string()
        };

        if current {
            if self.colors {
                style(styled).cyan().bold().to_string()
            } else {
                style(styled).bold().to_string()
            }
        } else {
            styled
        }
    }

    fn clear(&self) -> std::io::Result<()> {
        self.term.show_cursor()?;
        self.term
            .move_cursor_up(self.total_lines().saturating_sub(1))?;
        self.term.clear_to_end_of_screen()
    }
}

fn global_column_widths(
    updates: &BTreeMap<&Unit, Vec<Update<'_>>>,
    term: &Term,
) -> (usize, usize, usize) {
    let (name_w, cur_w, new_w) =
        updates
            .values()
            .flatten()
            .fold((0, 0, 0), |(name_w, cur_w, new_w), u| {
                let cur_len = u.current_req.to_string().len() + if u.yanked { 9 } else { 0 };
                (
                    name_w.max(u.name.len()),
                    cur_w.max(cur_len),
                    new_w.max(u.new_req.to_string().len()),
                )
            });

    let target = (term.size().1 as usize).min(40);
    let name_w = name_w.max(target.saturating_sub(cur_w + new_w + 4));

    (name_w, cur_w, new_w)
}
