//! Rendering for the agentboard sidebar. Ports `tui/components/*`
//! (StatusBar, SessionCard + AgentRow, DiffStats, status-visuals,
//! family-color, elapsed, short-model) to ratatui `Line`s.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use tt_agentboard::text::truncate;
use tt_agentboard::themes::ThemePalette;
use tt_agentboard::types::{AgentEvent, AgentStatus, MetadataTone, SessionData};

use super::{App, Modal, PanelFocus, SPINNERS, ToastTone};

const UNSEEN_ICON: &str = "●";

/// `#rrggbb` → ratatui color; `transparent`/garbage → None (terminal default).
fn hex(color: &str) -> Option<Color> {
    let s = color.strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

fn fg(color: &str) -> Style {
    match hex(color) {
        Some(c) => Style::default().fg(c),
        None => Style::default(),
    }
}

// --- Ported pure helpers ---

/// `family-color.ts`: stable per-project hue, slots collapse to one family.
fn family_color<'a>(session_name: &str, p: &'a ThemePalette) -> &'a str {
    let family = session_name
        .strip_suffix("-primary")
        .or_else(|| {
            session_name.rfind("-slot-").and_then(|i| {
                session_name[i + 6..]
                    .chars()
                    .all(|c| c.is_ascii_digit())
                    .then(|| &session_name[..i])
            })
        })
        .unwrap_or(session_name);
    let family = if family.is_empty() { session_name } else { family };

    match family {
        "blog" => p.pink,
        "dotfiles" => p.peach,
        "f" => p.teal,
        "toolbox" => p.sky,
        "towles-tool" => p.lavender,
        _ => {
            // Java-style 31-hash, matching the TS (i32 wrapping).
            let mut h: i32 = 0;
            for c in family.encode_utf16() {
                h = h.wrapping_mul(31).wrapping_add(c as i32);
            }
            let hues = [p.mauve, p.blue, p.green, p.yellow, p.red];
            hues[(h.unsigned_abs() as usize) % hues.len()]
        }
    }
}

/// `elapsed.ts`: 42s / 7m / 3h.
fn format_elapsed(ms: i64) -> String {
    let seconds = (ms.max(0)) / 1000;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    format!("{}h", minutes / 60)
}

/// `short-model.ts`: `claude-sonnet-5` → `sonnet-5`.
fn short_model(model: &str) -> String {
    let m = model.strip_prefix("claude-").unwrap_or(model);
    let m = if m.to_lowercase().ends_with("[1m]") { &m[..m.len() - 4] } else { m };
    m.to_string()
}

fn tone_color(tone: Option<MetadataTone>, p: &ThemePalette) -> &str {
    match tone {
        Some(MetadataTone::Success) => p.green,
        Some(MetadataTone::Error) => p.red,
        Some(MetadataTone::Warn) => p.yellow,
        Some(MetadataTone::Info) => p.blue,
        _ => p.overlay0,
    }
}

/// `status-visuals.ts` liveStatusIcon.
fn live_status_icon(status: AgentStatus, spin_idx: usize) -> &'static str {
    match status {
        AgentStatus::Busy => SPINNERS[spin_idx % SPINNERS.len()],
        AgentStatus::Waiting => "?",
        _ => "",
    }
}

fn unseen_terminal_color(status: AgentStatus, p: &ThemePalette) -> &str {
    match status {
        AgentStatus::Error => p.red,
        AgentStatus::Interrupted => p.peach,
        _ => p.teal,
    }
}

fn is_terminal(status: AgentStatus) -> bool {
    matches!(status, AgentStatus::Complete | AgentStatus::Error | AgentStatus::Interrupted)
}

// --- Frame layout ---

pub fn draw(frame: &mut Frame, app: &mut App) {
    let p = &app.theme.palette;
    let area = frame.area();

    if let Some(bg) = hex(p.crust) {
        frame.render_widget(Block::default().style(Style::default().bg(bg)), area);
    }

    let header_h = 3u16;
    let footer_h = footer_height(app);
    let list_area = Rect {
        x: area.x,
        y: area.y + header_h,
        width: area.width,
        height: area.height.saturating_sub(header_h + footer_h),
    };

    draw_status_bar(frame, app, Rect { height: header_h.min(area.height), ..area });
    draw_session_list(frame, app, list_area);
    draw_footer(
        frame,
        app,
        Rect {
            x: area.x,
            y: area.y + area.height.saturating_sub(footer_h),
            width: area.width,
            height: footer_h.min(area.height),
        },
    );

    match app.modal {
        Modal::ConfirmKill => draw_confirm_kill(frame, app, area),
        Modal::Help => draw_help(frame, app, area),
        Modal::None => {}
    }
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let p = &app.theme.palette;
    let busy: usize = app
        .sessions
        .iter()
        .map(|s| s.agents.iter().filter(|a| a.status == AgentStatus::Busy).count())
        .sum();
    let errors: usize = app
        .sessions
        .iter()
        .map(|s| s.agents.iter().filter(|a| a.status == AgentStatus::Error).count())
        .sum();
    let unseen = app.sessions.iter().filter(|s| s.unseen).count();

    let mut counts = vec![Span::styled(
        format!("  {}s", app.sessions.len()),
        fg(p.overlay0),
    )];
    if busy > 0 {
        counts.push(Span::styled(format!(" ⚡{busy}"), fg(p.yellow)));
    }
    if errors > 0 {
        counts.push(Span::styled(format!(" ✗{errors}"), fg(p.red)));
    }
    if unseen > 0 {
        counts.push(Span::styled(format!(" {UNSEEN_ICON}{unseen}"), fg(p.teal)));
    }

    let lines = vec![
        Line::default(),
        Line::from(Span::styled("  AgentBoard", fg(p.mauve).add_modifier(Modifier::BOLD))),
        Line::from(counts),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

fn divider(width: u16, color: &str) -> Line<'static> {
    Line::from(Span::styled("─".repeat(width as usize), fg(color)))
}

fn draw_session_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let p = &app.theme.palette;
    let mut lines: Vec<Line> = vec![divider(area.width, p.overlay0)];
    let mut focused_range: Option<(usize, usize)> = None;

    let current = app.current_session().map(str::to_string);
    let focused = app.focused_session.clone();

    for (i, session) in app.sessions.iter().enumerate() {
        if i > 0 {
            lines.push(divider(area.width, p.surface2));
        }
        let start = lines.len();
        let is_focused = focused.as_deref() == Some(session.name.as_str());
        let agent_focus = if is_focused && app.panel_focus == PanelFocus::Agents {
            app.focused_agent_idx as i64
        } else {
            -1
        };
        session_card(
            app,
            session,
            is_focused,
            current.as_deref() == Some(session.name.as_str()),
            agent_focus,
            &mut lines,
        );
        if is_focused {
            focused_range = Some((start, lines.len()));
        }
    }

    // Keep the focused card in view.
    let height = area.height as usize;
    if let Some((start, end)) = focused_range {
        let top = app.scroll as usize;
        if start < top {
            app.scroll = start as u16;
        } else if end > top + height {
            app.scroll = (end.saturating_sub(height)) as u16;
        }
    }
    let max_scroll = lines.len().saturating_sub(area.height as usize) as u16;
    app.scroll = app.scroll.min(max_scroll);

    frame.render_widget(Paragraph::new(lines).scroll((app.scroll, 0)), area);
}

/// One session card: accent bar + name/diff/status line, branch, metadata
/// summary, agent rows. Ports `SessionCard.tsx`.
fn session_card(
    app: &App,
    session: &SessionData,
    is_focused: bool,
    is_current: bool,
    focused_agent_idx: i64,
    lines: &mut Vec<Line<'static>>,
) {
    let theme = app.theme;
    let p = &theme.palette;
    let status = session.agent_state.as_ref().map(|a| a.status).unwrap_or(AgentStatus::Idle);
    let unseen_terminal = session.unseen && is_terminal(status);
    let busy_agents = session.agents.iter().filter(|a| a.status == AgentStatus::Busy).count();

    let accent: Option<&str> = if is_current {
        Some(p.green)
    } else if unseen_terminal {
        Some(unseen_terminal_color(status, p))
    } else {
        match status {
            AgentStatus::Error => Some(p.red),
            AgentStatus::Interrupted => Some(p.peach),
            AgentStatus::Busy => Some(p.yellow),
            AgentStatus::Waiting => Some(p.blue),
            _ if is_focused => Some(p.lavender),
            _ => None,
        }
    };

    let family_hue = family_color(&session.name, p);
    let card_bg = if is_focused { hex(p.surface0) } else { None };
    let bg_style = card_bg.map(|c| Style::default().bg(c)).unwrap_or_default();

    // Prefix: " ▌" accent or " ▎" dim family tick.
    let prefix = match accent {
        Some(color) => Span::styled(" ▌", fg(color).patch(bg_style)),
        None => Span::styled(" ▎", fg(family_hue).add_modifier(Modifier::DIM).patch(bg_style)),
    };

    let name_color = if is_focused {
        p.text
    } else if is_current {
        p.subtext1
    } else {
        family_hue
    };
    let mut name_style = fg(name_color).patch(bg_style);
    if is_focused || is_current {
        name_style = name_style.add_modifier(Modifier::BOLD);
    }

    // Header line: name + diff stats + status icon.
    let mut header = vec![
        prefix.clone(),
        Span::styled(truncate(&session.name, 18), name_style),
    ];
    if session.files_changed != 0
        || session.lines_added != 0
        || session.lines_removed != 0
        || session.commits_delta != 0
    {
        header.push(Span::styled(" ", bg_style));
        if session.files_changed != 0 {
            header.push(Span::styled(
                format!("{}f ", session.files_changed),
                fg(p.overlay0).patch(bg_style),
            ));
        }
        if session.lines_added != 0 {
            header.push(Span::styled(
                format!("+{} ", session.lines_added),
                fg(p.green).patch(bg_style),
            ));
        }
        if session.lines_removed != 0 {
            header.push(Span::styled(
                format!("-{} ", session.lines_removed),
                fg(p.red).patch(bg_style),
            ));
        }
        if session.commits_delta > 0 {
            header.push(Span::styled(
                format!("{}↑", session.commits_delta),
                fg(p.sky).patch(bg_style),
            ));
        } else if session.commits_delta < 0 {
            header.push(Span::styled(
                format!("{}↓", session.commits_delta.abs()),
                fg(p.peach).patch(bg_style),
            ));
        }
    }
    let status_icon = {
        let live = live_status_icon(status, app.spin_idx);
        if !live.is_empty() {
            live
        } else if unseen_terminal {
            UNSEEN_ICON
        } else {
            ""
        }
    };
    if !status_icon.is_empty() {
        let color = if unseen_terminal {
            unseen_terminal_color(status, p)
        } else {
            theme.status_color(status)
        };
        let count = if busy_agents > 1 { busy_agents.to_string() } else { String::new() };
        header.push(Span::styled(format!(" {status_icon}{count}"), fg(color).patch(bg_style)));
    }
    lines.push(Line::from(header).style(bg_style));

    // Branch line.
    if !session.branch.is_empty() {
        let color = if is_focused { p.pink } else { p.overlay0 };
        lines.push(
            Line::from(vec![
                Span::styled("   ", bg_style),
                Span::styled(truncate(&session.branch, 45), fg(color).patch(bg_style)),
            ])
            .style(bg_style),
        );
    }

    // Metadata summary line (status text · progress · label).
    if let Some(meta) = &session.metadata {
        let mut parts: Vec<String> = Vec::new();
        if let Some(status) = &meta.status {
            parts.push(status.text.clone());
        }
        if let Some(progress) = &meta.progress {
            match (progress.current, progress.total) {
                (Some(c), Some(t)) => parts.push(format!("{c}/{t}")),
                _ => {
                    if let Some(pct) = progress.percent {
                        parts.push(format!("{}%", (pct * 100.0).round() as i64));
                    }
                }
            }
            if let Some(label) = &progress.label {
                parts.push(label.clone());
            }
        }
        if !parts.is_empty() {
            let tone = meta.status.as_ref().and_then(|s| s.tone);
            lines.push(
                Line::from(vec![
                    Span::styled("   ", bg_style),
                    Span::styled(
                        parts.join(" · "),
                        fg(tone_color(tone, p)).add_modifier(Modifier::DIM).patch(bg_style),
                    ),
                ])
                .style(bg_style),
            );
        }
    }

    // Agent rows.
    for (i, agent) in session.agents.iter().enumerate() {
        agent_rows(app, agent, i as i64 == focused_agent_idx, card_bg, lines);
    }
}

/// One agent's rows: status/thread line, model·tool line, subagents, loop,
/// cache countdown. Ports `AgentRow`.
fn agent_rows(
    app: &App,
    agent: &AgentEvent,
    is_keyboard_focused: bool,
    card_bg: Option<Color>,
    lines: &mut Vec<Line<'static>>,
) {
    let theme = app.theme;
    let p = &theme.palette;
    let terminal = is_terminal(agent.status);
    let unseen = terminal && agent.unseen == Some(true);

    let row_bg = if is_keyboard_focused { hex(p.surface1) } else { card_bg };
    let bg_style = row_bg.map(|c| Style::default().bg(c)).unwrap_or_default();

    let icon = if unseen {
        UNSEEN_ICON
    } else if terminal {
        match agent.status {
            AgentStatus::Complete => "✓",
            AgentStatus::Error => "✗",
            _ => "⚠",
        }
    } else {
        let live = live_status_icon(agent.status, app.spin_idx);
        if live.is_empty() { "○" } else { live }
    };
    let color = if terminal {
        if unseen {
            unseen_terminal_color(agent.status, p)
        } else {
            match agent.status {
                AgentStatus::Error => p.red,
                AgentStatus::Interrupted => p.peach,
                _ => p.green,
            }
        }
    } else {
        theme.status_color(agent.status)
    };

    let mut row = vec![
        Span::styled("   ", bg_style),
        Span::styled(icon.to_string(), fg(color).patch(bg_style)),
    ];
    if let Some(thread_name) = &agent.thread_name {
        let name_color = if unseen { color } else { p.overlay0 };
        let compact = thread_name.split_whitespace().collect::<Vec<_>>().join(" ");
        row.push(Span::styled(
            format!(" {}", truncate(&compact, 40)),
            fg(name_color).patch(bg_style),
        ));
    }
    if agent.status == AgentStatus::Busy
        && let Some(last) = agent.details.as_ref().and_then(|d| d.last_activity_at)
    {
        let color = if is_keyboard_focused { p.subtext0 } else { p.overlay1 };
        row.push(Span::styled(
            format!(" {}", format_elapsed(app.now_ms - last)),
            fg(color).add_modifier(Modifier::DIM).patch(bg_style),
        ));
    }
    lines.push(Line::from(row).style(bg_style));

    let Some(details) = &agent.details else {
        return;
    };

    // model · ⟶ tool
    if agent.status == AgentStatus::Busy {
        let model = details.model.as_deref().map(short_model).unwrap_or_default();
        let tool = details.last_tool.as_deref().unwrap_or("");
        if !model.is_empty() || !tool.is_empty() {
            let mut spans = vec![Span::styled("    ", bg_style)];
            if !model.is_empty() {
                spans.push(Span::styled(
                    model.clone(),
                    fg(p.subtext0).add_modifier(Modifier::DIM).patch(bg_style),
                ));
            }
            if !tool.is_empty() {
                if !model.is_empty() {
                    spans.push(Span::styled(
                        " · ",
                        fg(p.overlay0).add_modifier(Modifier::DIM).patch(bg_style),
                    ));
                }
                spans.push(Span::styled(
                    "⟶ ",
                    fg(p.teal).add_modifier(Modifier::DIM).patch(bg_style),
                ));
                spans.push(Span::styled(tool.to_string(), fg(p.subtext0).patch(bg_style)));
            }
            lines.push(Line::from(spans).style(bg_style));
        }

        // Subagents.
        if let Some(subagents) = &details.subagents
            && !subagents.is_empty()
        {
            let plural = if subagents.len() == 1 { "" } else { "s" };
            lines.push(
                Line::from(vec![
                    Span::styled("    ", bg_style),
                    Span::styled("⚡ ", fg(p.mauve).add_modifier(Modifier::DIM).patch(bg_style)),
                    Span::styled(
                        format!("{} agent{plural}", subagents.len()),
                        fg(p.subtext0).patch(bg_style),
                    ),
                ])
                .style(bg_style),
            );
            for sa in subagents {
                let mut spans = vec![Span::styled(
                    "      ↳ ",
                    fg(p.overlay0).add_modifier(Modifier::DIM).patch(bg_style),
                )];
                if let Some(agent_type) = &sa.agent_type {
                    spans.push(Span::styled(
                        agent_type.clone(),
                        fg(p.teal).add_modifier(Modifier::DIM).patch(bg_style),
                    ));
                }
                if let Some(description) = &sa.description {
                    if sa.agent_type.is_some() {
                        spans.push(Span::styled(
                            " · ",
                            fg(p.overlay0).add_modifier(Modifier::DIM).patch(bg_style),
                        ));
                    }
                    let compact = description.split_whitespace().collect::<Vec<_>>().join(" ");
                    spans
                        .push(Span::styled(truncate(&compact, 40), fg(p.subtext0).patch(bg_style)));
                }
                lines.push(Line::from(spans).style(bg_style));
            }
        }
    }

    // Loop wakeup countdown.
    if let Some(l) = &details.r#loop
        && l.next_wake_at > app.now_ms
    {
        let mut spans = vec![
            Span::styled("    ", bg_style),
            Span::styled("⟳ ", fg(p.lavender).add_modifier(Modifier::DIM).patch(bg_style)),
            Span::styled(
                format!("loops in {}", format_elapsed(l.next_wake_at - app.now_ms)),
                fg(p.subtext0).patch(bg_style),
            ),
        ];
        if let Some(reason) = &l.reason {
            let compact = reason.split_whitespace().collect::<Vec<_>>().join(" ");
            spans.push(Span::styled(
                format!(" · {}", truncate(&compact, 36)),
                fg(p.overlay0).add_modifier(Modifier::DIM).patch(bg_style),
            ));
        }
        lines.push(Line::from(spans).style(bg_style));
    }

    // Cache countdown.
    let expires_at =
        details.cache_expires_at.or(details.last_activity_at.map(|t| t + 60 * 60 * 1000));
    if let Some(expires_at) = expires_at {
        let minutes_left = (expires_at - app.now_ms).div_euclid(60_000) + 1;
        let label = if minutes_left <= 0 {
            "cache expired".to_string()
        } else {
            format!("cache {minutes_left}m")
        };
        lines.push(
            Line::from(vec![
                Span::styled("    ", bg_style),
                Span::styled(label, fg(p.overlay0).add_modifier(Modifier::DIM).patch(bg_style)),
            ])
            .style(bg_style),
        );
    }
}

fn footer_height(app: &App) -> u16 {
    let toast = if app.toast.is_some() { 1 } else { 0 };
    let hints = if app.panel_focus == PanelFocus::Agents { 2 } else { 1 };
    2 + toast + hints
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let p = &app.theme.palette;
    let mut lines = vec![divider(area.width, p.surface2)];

    if let Some((message, tone, _)) = &app.toast {
        let color = match tone {
            ToastTone::Error => p.red,
            ToastTone::Success => p.green,
            ToastTone::Info => p.blue,
        };
        lines.push(Line::from(Span::styled(format!(" {message}"), fg(color))));
    }

    if app.panel_focus == PanelFocus::Agents {
        lines.push(Line::from(vec![
            Span::styled(" ←", fg(p.overlay0)),
            Span::styled(" back        ", fg(p.overlay1)),
            Span::styled("⏎", fg(p.overlay0)),
            Span::styled(" focus", fg(p.overlay1)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(" d", fg(p.overlay0)),
            Span::styled(" dismiss     ", fg(p.overlay1)),
            Span::styled("x", fg(p.overlay0)),
            Span::styled(" kill", fg(p.overlay1)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled(" ?", fg(p.overlay0)),
            Span::styled(" help", fg(p.overlay1)),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn draw_confirm_kill(frame: &mut Frame, app: &App, area: Rect) {
    let p = &app.theme.palette;
    let target = app.kill_target.clone().unwrap_or_default();
    let rect = centered(area, (target.len() as u16 + 8).max(20), 5);
    frame.render_widget(Clear, rect);

    let mut block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded);
    if let Some(c) = hex(p.red) {
        block = block.border_style(Style::default().fg(c));
    }
    if let Some(bg) = hex(p.mantle) {
        block = block.style(Style::default().bg(bg));
    }
    let lines = vec![
        Line::from(Span::styled("Kill session?", fg(p.red).add_modifier(Modifier::BOLD)))
            .centered(),
        Line::from(Span::styled(target, fg(p.text))).centered(),
        Line::from(vec![
            Span::styled("y", fg(p.overlay0)),
            Span::styled("/", fg(p.overlay1)),
            Span::styled("n", fg(p.overlay0)),
        ])
        .centered(),
    ];
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

const HELP_KEYS: [(&str, &str); 13] = [
    ("j/k ↑↓", "Move focus"),
    ("Enter", "Switch to session"),
    ("1-9", "Jump to session"),
    ("Tab", "Cycle sessions"),
    ("n", "New session"),
    ("e", "Open in editor"),
    ("x", "Kill session"),
    ("r", "Refresh"),
    ("→/l", "Agents panel"),
    ("←/h/Esc", "Back to sessions"),
    ("Alt+↑↓", "Reorder sessions"),
    ("Alt+Shift+↑↓", "To top/bottom"),
    ("q", "Quit"),
];

fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    let p = &app.theme.palette;
    let rect = centered(area, 34, HELP_KEYS.len() as u16 + 6);
    frame.render_widget(Clear, rect);

    let mut block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded);
    if let Some(c) = hex(p.blue) {
        block = block.border_style(Style::default().fg(c));
    }
    if let Some(bg) = hex(p.mantle) {
        block = block.style(Style::default().bg(bg));
    }

    let mut lines = vec![
        Line::from(Span::styled("Keybindings", fg(p.blue).add_modifier(Modifier::BOLD))),
        divider(rect.width.saturating_sub(2), p.surface2),
    ];
    for (key, desc) in HELP_KEYS {
        lines.push(Line::from(vec![
            Span::styled(format!("{key:<14}"), fg(p.sky)),
            Span::styled(desc, fg(p.subtext0)),
        ]));
    }
    lines.push(divider(rect.width.saturating_sub(2), p.surface2));
    lines.push(Line::from(Span::styled(
        "Press any key to close",
        fg(p.overlay0).add_modifier(Modifier::DIM),
    )));

    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parses_and_transparent_is_none() {
        assert_eq!(hex("#1e1e2e"), Some(Color::Rgb(0x1e, 0x1e, 0x2e)));
        assert_eq!(hex("transparent"), None);
        assert_eq!(hex("#zzz"), None);
    }

    #[test]
    fn elapsed_matches_ts() {
        assert_eq!(format_elapsed(-5), "0s");
        assert_eq!(format_elapsed(42_000), "42s");
        assert_eq!(format_elapsed(7 * 60_000), "7m");
        assert_eq!(format_elapsed(3 * 3_600_000), "3h");
    }

    #[test]
    fn short_model_strips_prefix_and_1m_suffix() {
        assert_eq!(short_model("claude-sonnet-5"), "sonnet-5");
        assert_eq!(short_model("claude-opus-4[1m]"), "opus-4");
        assert_eq!(short_model("gpt-5"), "gpt-5");
    }

    #[test]
    fn family_color_strips_slot_suffixes_and_is_stable() {
        let p = &tt_agentboard::themes::resolve_theme(None).palette;
        // Known family (towles-tool) via -primary and -slot-N collapse.
        assert_eq!(family_color("towles-tool-primary", p), p.lavender);
        assert_eq!(family_color("towles-tool-slot-3", p), p.lavender);
        assert_eq!(family_color("blog", p), p.pink);
        // Unknown families hash deterministically.
        assert_eq!(family_color("mystery-slot-1", p), family_color("mystery", p));
    }
}
