use std::collections::BTreeMap;

use check_updates::{Package, Unit, Usage};
use console::{Style, Term, style};
use dialoguer::MultiSelect;
use dialoguer::theme::ColorfulTheme;
use semver::VersionReq;

use crate::update::{Update, format_update_line};

pub fn prompt_updates<'a>(
    updates: &BTreeMap<&'a Unit, Vec<Update<'a>>>,
) -> std::io::Result<Vec<(&'a Usage, &'a Package, VersionReq)>> {
    enum SelectionItem<'a> {
        Header(String),
        Update {
            label: String,
            usage: &'a Usage,
            package: &'a Package,
            new_req: VersionReq,
        },
    }

    let mut items: Vec<SelectionItem<'a>> = Vec::new();
    let mut defaults: Vec<bool> = Vec::new();

    let (name_w, cur_w, new_w) = global_column_widths(updates);
    for (unit, unit_updates) in updates {
        if updates.len() > 1 {
            let header = Style::new().bold().apply_to(unit.name()).to_string();
            items.push(SelectionItem::Header(header));
            defaults.push(false);
        }

        for update in unit_updates {
            let label = format_update_line(update, name_w, cur_w, new_w);
            items.push(SelectionItem::Update {
                label,
                usage: update.usage,
                package: update.package,
                new_req: update.new_req.clone(),
            });
            defaults.push(true);
        }
    }

    let display_items: Vec<&str> = items
        .iter()
        .map(|item| match item {
            SelectionItem::Header(h) => h.as_str(),
            SelectionItem::Update { label, .. } => label.as_str(),
        })
        .collect();

    let term_height = Term::stderr().size().0 as usize;
    let max_length = term_height.saturating_sub(5);

    let Some(selected_indices) = MultiSelect::with_theme(&custom_theme())
        .with_prompt("Choose which packages to update")
        .report(false)
        .items(&display_items)
        .defaults(&defaults)
        .max_length(max_length)
        .interact_opt()?
    else {
        return Ok(Vec::new());
    };

    Ok(selected_indices
        .into_iter()
        .filter_map(|idx| match &items[idx] {
            SelectionItem::Header(_) => None,
            SelectionItem::Update {
                usage,
                package,
                new_req,
                ..
            } => Some((*usage, *package, new_req.clone())),
        })
        .collect())
}

fn global_column_widths(updates: &BTreeMap<&Unit, Vec<Update<'_>>>) -> (usize, usize, usize) {
    let mut name_w = 0;
    let mut cur_w = 0;
    let mut new_w = 0;

    for unit_updates in updates.values() {
        for u in unit_updates {
            name_w = name_w.max(u.name.len());
            let cur_len = u.current_req.to_string().len() + if u.yanked { 9 } else { 0 };
            cur_w = cur_w.max(cur_len);
            new_w = new_w.max(u.new_req.to_string().len());
        }
    }

    let total = name_w + cur_w + new_w + 4;
    let target = (Term::stdout().size().1 as usize).min(40);
    if total < target {
        name_w += target - total;
    }
    (name_w, cur_w, new_w)
}

fn custom_theme() -> ColorfulTheme {
    let colors_supported = Term::stderr().features().colors_supported();

    let checked = if colors_supported {
        style("◉".to_string()).green()
    } else {
        style("[x]".to_string())
    };

    let unchecked = if colors_supported {
        style("◯".to_string())
    } else {
        style("[ ]".to_string())
    };

    ColorfulTheme {
        checked_item_prefix: checked,
        unchecked_item_prefix: unchecked,
        active_item_style: if colors_supported {
            Style::new().cyan()
        } else {
            Style::new()
        },
        inactive_item_style: Style::new(),
        prompt_style: if colors_supported {
            Style::new().bold()
        } else {
            Style::new()
        },
        ..ColorfulTheme::default()
    }
}
