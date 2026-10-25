use std::collections::HashMap;
use std::io::{Stdout, Write, stdout};

use anyhow::{Context, Result};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{
    Clear, ClearType, DisableLineWrap, EnableLineWrap, EnterAlternateScreen, LeaveAlternateScreen,
    disable_raw_mode, enable_raw_mode,
};
use crossterm::{ExecutableCommand, QueueableCommand};
use tastty_core::{ColorRole, Position, TerminalMode};

use crate::clipboard::Clipboard;
use crate::config::AppearanceConfig;
use crate::launcher::Launcher;
use crate::pane::Pane;
use crate::tiling::{Orientation, PaneGeometry, PaneId, Rect};
use crate::workspace::WORKSPACE_COUNT;

const BACKGROUND: Color = Color::Rgb {
    r: 0x1d,
    g: 0x1f,
    b: 0x21,
};
const FOREGROUND: Color = Color::Rgb {
    r: 0xc5,
    g: 0xc8,
    b: 0xc6,
};
#[cfg(test)]
const FOCUSED_BORDER: Color = Color::Rgb {
    r: 0x39,
    g: 0x71,
    b: 0xed,
};
const SELECTION: Color = Color::Rgb {
    r: 0x24,
    g: 0x4a,
    b: 0x99,
};
const SPLIT_INDICATOR: Color = Color::Rgb {
    r: 0x4c,
    g: 0xb7,
    b: 0xd8,
};
const ERROR: Color = Color::Rgb {
    r: 0xcc,
    g: 0x34,
    b: 0x2b,
};
const SEARCH_MATCH: Color = Color::Rgb {
    r: 0xf0,
    g: 0xc6,
    b: 0x74,
};

pub struct TerminalRenderer {
    output: Stdout,
    previous: Vec<RenderCell>,
    previous_size: (u16, u16),
    focused_border: Color,
    unfocused_border: Color,
    clipboard: Clipboard,
}

#[derive(Clone, Copy, Default)]
pub struct RenderOverlays<'a> {
    pub mode_label: Option<&'a str>,
    pub launcher: Option<&'a Launcher>,
    pub notice: Option<&'a str>,
    pub workspace_bar: Option<WorkspaceBar>,
    pub split_orientation: Option<Orientation>,
}

#[derive(Clone, Copy)]
pub struct WorkspaceBar {
    pub active: usize,
    pub open: [bool; WORKSPACE_COUNT],
}

impl TerminalRenderer {
    pub fn enter(appearance: &AppearanceConfig) -> Result<Self> {
        let focused_border = parse_color(&appearance.focused_border)?;
        let unfocused_border = parse_color(&appearance.unfocused_border)?;
        enable_raw_mode().context("failed to enable terminal raw mode")?;
        let mut output = stdout();
        if let Err(error) = output
            .execute(EnterAlternateScreen)
            .and_then(|output| output.execute(DisableLineWrap))
            .and_then(|output| output.execute(EnableBracketedPaste))
            .and_then(|output| output.execute(EnableMouseCapture))
            .and_then(|output| output.execute(Hide))
        {
            let _ = output.execute(DisableMouseCapture);
            let _ = output.execute(DisableBracketedPaste);
            let _ = output.execute(Show);
            let _ = output.execute(EnableLineWrap);
            let _ = output.execute(LeaveAlternateScreen);
            let _ = output.flush();
            let _ = disable_raw_mode();
            return Err(error).context("failed to initialize terminal display");
        }
        Ok(Self {
            output,
            previous: Vec::new(),
            previous_size: (0, 0),
            focused_border,
            unfocused_border,
            clipboard: Clipboard::new(),
        })
    }

    pub fn update_appearance(&mut self, appearance: &AppearanceConfig) -> Result<()> {
        self.focused_border = parse_color(&appearance.focused_border)?;
        self.unfocused_border = parse_color(&appearance.unfocused_border)?;
        Ok(())
    }

    pub fn draw_panes(
        &mut self,
        panes: &HashMap<PaneId, Pane>,
        layout: &[PaneGeometry],
        focused: Option<PaneId>,
        bounds: Rect,
        overlays: RenderOverlays<'_>,
    ) -> Result<()> {
        let mut canvas = Canvas::new(bounds.width, bounds.height);
        for geometry in layout {
            if let Some(pane) = panes.get(&geometry.pane) {
                canvas.draw_pane_contents(pane, geometry.rect);
            }
        }
        for geometry in layout
            .iter()
            .filter(|geometry| focused != Some(geometry.pane))
        {
            canvas.draw_border(geometry.rect, self.unfocused_border);
        }
        if let Some(geometry) = layout
            .iter()
            .find(|geometry| focused == Some(geometry.pane))
        {
            canvas.draw_border(geometry.rect, self.focused_border);
        }
        if let Some(orientation) = overlays.split_orientation
            && let Some(geometry) = layout
                .iter()
                .find(|geometry| Some(geometry.pane) == focused)
        {
            canvas.draw_split_indicator(geometry.rect, orientation);
        }
        if let Some(launcher) = overlays.launcher {
            canvas.draw_launcher(bounds, launcher);
        }
        if let Some(notice) = overlays.notice {
            canvas.draw_notice(bounds, notice);
        }
        if let Some(workspaces) = overlays.workspace_bar {
            canvas.draw_workspace_bar(bounds, workspaces, self.unfocused_border);
        }
        if let Some(label) = overlays.mode_label {
            canvas.draw_mode_label(bounds, label);
        }

        self.output.queue(Hide)?;
        self.draw_changes(&canvas)?;
        self.output
            .queue(SetAttribute(Attribute::Reset))?
            .queue(ResetColor)?;
        if overlays.mode_label.is_none()
            && overlays.launcher.is_none()
            && let Some((cursor_x, cursor_y)) = focused_cursor(panes, layout, focused)
        {
            self.output.queue(MoveTo(cursor_x, cursor_y))?.queue(Show)?;
        } else {
            self.output.queue(Hide)?;
        }
        self.output.flush().context("failed to render terminal")
    }

    pub fn copy_to_clipboard(&mut self, text: &str) -> Result<()> {
        self.clipboard.copy(text)
    }

    fn draw_changes(&mut self, canvas: &Canvas) -> Result<()> {
        let resized = self.previous_size != (canvas.width, canvas.height);
        if resized {
            self.output.queue(Clear(ClearType::All))?;
            self.previous = vec![RenderCell::default(); canvas.cells.len()];
            self.previous_size = (canvas.width, canvas.height);
        }
        for (index, cell) in canvas.cells.iter().enumerate() {
            if !resized && self.previous.get(index) == Some(cell) {
                continue;
            }
            if cell.wide_continuation {
                continue;
            }
            let width = usize::from(canvas.width);
            if width == 0 {
                break;
            }
            let x = u16::try_from(index % width).unwrap_or(u16::MAX);
            let y = u16::try_from(index / width).unwrap_or(u16::MAX);
            self.output
                .queue(MoveTo(x, y))?
                .queue(SetAttribute(Attribute::Reset))?
                .queue(SetForegroundColor(cell.foreground))?
                .queue(SetBackgroundColor(cell.background))?;
            if cell.bold {
                self.output.queue(SetAttribute(Attribute::Bold))?;
            }
            if cell.dim {
                self.output.queue(SetAttribute(Attribute::Dim))?;
            }
            if cell.italic {
                self.output.queue(SetAttribute(Attribute::Italic))?;
            }
            if cell.underline {
                self.output.queue(SetAttribute(Attribute::Underlined))?;
            }
            self.output.queue(Print(&cell.text))?;
        }
        self.previous.clone_from(&canvas.cells);
        Ok(())
    }
}

impl Drop for TerminalRenderer {
    fn drop(&mut self) {
        let _ = self.output.execute(SetAttribute(Attribute::Reset));
        let _ = self.output.execute(ResetColor);
        let _ = self.output.execute(Show);
        let _ = self.output.execute(DisableMouseCapture);
        let _ = self.output.execute(DisableBracketedPaste);
        let _ = self.output.execute(EnableLineWrap);
        let _ = self.output.execute(LeaveAlternateScreen);
        let _ = self.output.flush();
        let _ = disable_raw_mode();
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RenderCell {
    text: String,
    foreground: Color,
    background: Color,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    wide_continuation: bool,
}

impl Default for RenderCell {
    fn default() -> Self {
        Self {
            text: " ".to_owned(),
            foreground: FOREGROUND,
            background: BACKGROUND,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            wide_continuation: false,
        }
    }
}

struct Canvas {
    width: u16,
    height: u16,
    cells: Vec<RenderCell>,
}

impl Canvas {
    fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            cells: vec![RenderCell::default(); usize::from(width) * usize::from(height)],
        }
    }

    fn set(&mut self, x: u16, y: u16, cell: RenderCell) {
        if x >= self.width || y >= self.height {
            return;
        }
        let index = usize::from(y) * usize::from(self.width) + usize::from(x);
        self.cells[index] = cell;
    }

    fn styled_cell(text: impl Into<String>, foreground: Color, background: Color) -> RenderCell {
        RenderCell {
            text: text.into(),
            foreground,
            background,
            ..RenderCell::default()
        }
    }

    fn draw_border(&mut self, rect: Rect, color: Color) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        let right = rect.x.saturating_add(rect.width.saturating_sub(1));
        let bottom = rect.y.saturating_add(rect.height.saturating_sub(1));
        for x in rect.x..=right {
            self.set(x, rect.y, Self::styled_cell("─", color, BACKGROUND));
            self.set(x, bottom, Self::styled_cell("─", color, BACKGROUND));
        }
        for y in rect.y..=bottom {
            self.set(rect.x, y, Self::styled_cell("│", color, BACKGROUND));
            self.set(right, y, Self::styled_cell("│", color, BACKGROUND));
        }
        self.set(rect.x, rect.y, Self::styled_cell("┌", color, BACKGROUND));
        self.set(right, rect.y, Self::styled_cell("┐", color, BACKGROUND));
        self.set(rect.x, bottom, Self::styled_cell("└", color, BACKGROUND));
        self.set(right, bottom, Self::styled_cell("┘", color, BACKGROUND));
    }

    fn draw_pane_contents(&mut self, pane: &Pane, rect: Rect) {
        let visible_columns = rect.width.saturating_sub(2);
        let visible_rows = rect.height.saturating_sub(2);
        if visible_columns == 0 || visible_rows == 0 {
            return;
        }
        let screen = pane.screen();
        let size = screen.size();
        let rows = visible_rows.min(size.rows);
        let columns = visible_columns.min(size.cols);
        for row in 0..rows {
            for column in 0..columns {
                let Some(cell) = screen.cell(row, column) else {
                    continue;
                };
                let attrs = cell.attrs();
                let inverse = attrs.inverse() ^ screen.mode(TerminalMode::ReverseVideo);
                let (foreground, background) = if inverse {
                    (attrs.bg_color, attrs.fg_color)
                } else {
                    (attrs.fg_color, attrs.bg_color)
                };
                let (foreground, background) = if cell.has_contents()
                    && cell
                        .contents()
                        .chars()
                        .any(|character| !character.is_whitespace())
                    && pane.selection_contains(Position::new(row, column))
                {
                    (FOREGROUND, SELECTION)
                } else {
                    (
                        map_color(screen, foreground, ColorRole::Foreground, FOREGROUND),
                        map_color(screen, background, ColorRole::Background, BACKGROUND),
                    )
                };
                self.set(
                    rect.x.saturating_add(1).saturating_add(column),
                    rect.y.saturating_add(1).saturating_add(row),
                    RenderCell {
                        text: if cell.has_contents() {
                            cell.contents().to_owned()
                        } else {
                            " ".to_owned()
                        },
                        foreground,
                        background,
                        bold: attrs.bold(),
                        dim: attrs.dim(),
                        italic: attrs.italic(),
                        underline: attrs.underline(),
                        wide_continuation: cell.is_wide_continuation(),
                    },
                );
            }
        }
    }

    fn draw_split_indicator(&mut self, rect: Rect, orientation: Orientation) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        let right = rect.x.saturating_add(rect.width.saturating_sub(1));
        let bottom = rect.y.saturating_add(rect.height.saturating_sub(1));
        match orientation {
            Orientation::Horizontal => {
                for y in rect.y.saturating_add(1)..bottom {
                    self.set(
                        right,
                        y,
                        Self::styled_cell("│", SPLIT_INDICATOR, BACKGROUND),
                    );
                }
            }
            Orientation::Vertical => {
                for x in rect.x.saturating_add(1)..right {
                    self.set(
                        x,
                        bottom,
                        Self::styled_cell("─", SPLIT_INDICATOR, BACKGROUND),
                    );
                }
            }
        }
    }

    fn draw_mode_label(&mut self, bounds: Rect, label: &str) {
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        let text = clipped_text(label, usize::from(bounds.width));
        let text_width = unicode_width::UnicodeWidthStr::width(text.as_str());
        let x = bounds
            .x
            .saturating_add(bounds.width.saturating_sub(text_width as u16));
        self.draw_text(x, bounds.y, text_width, &text, FOREGROUND, SELECTION);
    }

    fn draw_workspace_bar(
        &mut self,
        bounds: Rect,
        workspaces: WorkspaceBar,
        inactive_background: Color,
    ) {
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        self.draw_text(
            bounds.x,
            bounds.y,
            usize::from(bounds.width),
            "",
            FOREGROUND,
            BACKGROUND,
        );
        let mut x = bounds.x;
        for (index, open) in workspaces.open.into_iter().enumerate() {
            if !open || x >= bounds.x.saturating_add(bounds.width) {
                continue;
            }
            let label = format!(" {} ", index + 1);
            let width = label.chars().count();
            self.draw_text(
                x,
                bounds.y,
                width,
                &label,
                FOREGROUND,
                if index == workspaces.active {
                    SELECTION
                } else {
                    inactive_background
                },
            );
            x = x.saturating_add(u16::try_from(width).unwrap_or(u16::MAX));
        }
    }

    fn draw_notice(&mut self, bounds: Rect, notice: &str) {
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        self.draw_text(
            bounds.x,
            bounds.y.saturating_add(bounds.height.saturating_sub(1)),
            usize::from(bounds.width),
            notice,
            FOREGROUND,
            ERROR,
        );
    }

    fn draw_launcher(&mut self, bounds: Rect, launcher: &Launcher) {
        if bounds.width < 3 || bounds.height < 3 {
            return;
        }
        let desired_width =
            u32::from(bounds.width).saturating_mul(u32::from(launcher.width_percent())) / 100;
        let width = (desired_width as u16).max(20).min(bounds.width);
        let entries =
            launcher.visible_entries_with_limit(usize::from(bounds.height.saturating_sub(3)));
        let result_rows = entries.len().max(1).min(launcher.max_results());
        let desired_height = u16::try_from(result_rows.saturating_add(4)).unwrap_or(u16::MAX);
        let height = desired_height.max(5).min(bounds.height);
        let rect = Rect::new(
            bounds
                .x
                .saturating_add(bounds.width.saturating_sub(width) / 2),
            bounds
                .y
                .saturating_add(bounds.height.saturating_sub(height) / 2),
            width,
            height,
        );
        self.fill_rect(rect, BACKGROUND);
        self.draw_border(rect, SELECTION);
        let inner_width = usize::from(rect.width.saturating_sub(2));
        self.draw_text(
            rect.x.saturating_add(1),
            rect.y,
            inner_width,
            &format!(" {} ", launcher.mode_name()),
            FOREGROUND,
            BACKGROUND,
        );
        self.draw_text(
            rect.x.saturating_add(1),
            rect.y.saturating_add(1),
            inner_width,
            &format!("Search: {}_", launcher.query()),
            FOREGROUND,
            BACKGROUND,
        );
        if let Some(status) = launcher.status() {
            self.draw_text(
                rect.x.saturating_add(1),
                rect.y.saturating_add(2),
                inner_width,
                status,
                ERROR,
                BACKGROUND,
            );
            return;
        }
        if entries.is_empty() {
            self.draw_text(
                rect.x.saturating_add(1),
                rect.y.saturating_add(2),
                inner_width,
                "No matches",
                FOREGROUND,
                BACKGROUND,
            );
            return;
        }
        for (row, (selected, entry)) in entries.into_iter().enumerate() {
            let y = rect
                .y
                .saturating_add(2)
                .saturating_add(u16::try_from(row).unwrap_or(u16::MAX));
            if y >= rect.y.saturating_add(rect.height.saturating_sub(1)) {
                break;
            }
            let description = entry.description.as_deref().unwrap_or("");
            let separator = if description.is_empty() { "" } else { " — " };
            self.draw_text(
                rect.x.saturating_add(1),
                y,
                inner_width,
                &format!(
                    "{}{}{}{}",
                    if selected { "> " } else { "  " },
                    entry.label,
                    separator,
                    description
                ),
                FOREGROUND,
                if selected { SELECTION } else { BACKGROUND },
            );
            self.highlight_matches(
                rect.x.saturating_add(3),
                y,
                inner_width.saturating_sub(2),
                &entry.label,
                launcher.match_indices(entry),
            );
        }
    }

    fn highlight_matches(&mut self, x: u16, y: u16, width: usize, text: &str, indices: &[u32]) {
        use unicode_segmentation::UnicodeSegmentation;
        use unicode_width::UnicodeWidthStr;
        if indices.is_empty() || y >= self.height {
            return;
        }
        let width = width.min(usize::from(self.width.saturating_sub(x)));
        let mut sanitized = String::new();
        let mut matched_characters = Vec::new();
        let ascii = text.is_ascii();
        let mut index = 0;
        for grapheme in text.graphemes(true) {
            let matched = indices.binary_search(&(index as u32)).is_ok();
            for character in grapheme.chars().filter(|character| !character.is_control()) {
                sanitized.push(character);
                matched_characters.push(matched);
            }
            index += if ascii { grapheme.len() } else { 1 };
        }
        let mut column = 0;
        let mut character_index = 0;
        for grapheme in sanitized.graphemes(true) {
            let columns = grapheme.width();
            if column + columns > width {
                break;
            }
            let end = character_index + grapheme.chars().count();
            if matched_characters[character_index..end]
                .iter()
                .any(|matched| *matched)
            {
                for offset in 0..columns {
                    let cell = &mut self.cells[usize::from(y) * usize::from(self.width)
                        + usize::from(x)
                        + column
                        + offset];
                    cell.foreground = SEARCH_MATCH;
                    cell.bold = true;
                }
            }
            character_index = end;
            column += columns;
        }
    }

    fn fill_rect(&mut self, rect: Rect, background: Color) {
        for y in rect.y..rect.y.saturating_add(rect.height) {
            for x in rect.x..rect.x.saturating_add(rect.width) {
                self.set(x, y, Self::styled_cell(" ", FOREGROUND, background));
            }
        }
    }

    fn draw_text(
        &mut self,
        x: u16,
        y: u16,
        width: usize,
        text: &str,
        foreground: Color,
        background: Color,
    ) {
        use unicode_segmentation::UnicodeSegmentation;
        use unicode_width::UnicodeWidthStr;
        let width = width.min(usize::from(self.width.saturating_sub(x)));
        let text = clipped_text(text, width);
        let mut offset = 0;
        for grapheme in text.graphemes(true) {
            self.set(
                x.saturating_add(u16::try_from(offset).unwrap_or(u16::MAX)),
                y,
                Self::styled_cell(grapheme, foreground, background),
            );
            for continuation in 1..grapheme.width() {
                let mut cell = Self::styled_cell("", foreground, background);
                cell.wide_continuation = true;
                self.set(x + (offset + continuation) as u16, y, cell);
            }
            offset += grapheme.width();
        }
        for offset in offset..width {
            self.set(
                x.saturating_add(u16::try_from(offset).unwrap_or(u16::MAX)),
                y,
                Self::styled_cell(" ", foreground, background),
            );
        }
    }
}

fn clipped_text(text: &str, width: usize) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    let sanitized: String = text
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    let mut remaining = width;
    sanitized
        .graphemes(true)
        .take_while(|grapheme| {
            let columns = grapheme.width();
            if columns > remaining {
                return false;
            }
            remaining -= columns;
            true
        })
        .filter(|grapheme| grapheme.width() > 0)
        .collect()
}

fn focused_cursor(
    panes: &HashMap<PaneId, Pane>,
    layout: &[PaneGeometry],
    focused: Option<PaneId>,
) -> Option<(u16, u16)> {
    let focused = focused?;
    let pane = panes.get(&focused)?;
    let geometry = layout.iter().find(|geometry| geometry.pane == focused)?;
    if pane.screen().mode(TerminalMode::HideCursor)
        || geometry.rect.width < 3
        || geometry.rect.height < 3
    {
        return None;
    }
    let cursor = pane.screen().cursor();
    let max_column = geometry.rect.width.saturating_sub(3);
    let max_row = geometry.rect.height.saturating_sub(3);
    Some((
        geometry
            .rect
            .x
            .saturating_add(1)
            .saturating_add(cursor.col.min(max_column)),
        geometry
            .rect
            .y
            .saturating_add(1)
            .saturating_add(cursor.row.min(max_row)),
    ))
}

fn map_color(
    screen: &tastty_core::Screen,
    color: tastty_core::Color,
    role: ColorRole,
    default: Color,
) -> Color {
    match color {
        tastty_core::Color::Default => default,
        tastty_core::Color::Index(_) | tastty_core::Color::Rgb(_, _, _) => {
            let (red, green, blue) = screen.resolve_color(color, role);
            Color::Rgb {
                r: red,
                g: green,
                b: blue,
            }
        }
    }
}

fn parse_color(value: &str) -> Result<Color> {
    let bytes = value.as_bytes();
    if bytes.len() != 7 || bytes[0] != b'#' || !bytes[1..].iter().all(u8::is_ascii_hexdigit) {
        anyhow::bail!("invalid color {value:?}; expected #RRGGBB");
    }
    let red = u8::from_str_radix(&value[1..3], 16)
        .with_context(|| format!("invalid red component in {value}"))?;
    let green = u8::from_str_radix(&value[3..5], 16)
        .with_context(|| format!("invalid green component in {value}"))?;
    let blue = u8::from_str_radix(&value[5..7], 16)
        .with_context(|| format!("invalid blue component in {value}"))?;
    Ok(Color::Rgb {
        r: red,
        g: green,
        b: blue,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_highlights_preserve_selection_and_unicode_geometry() {
        for background in [BACKGROUND, SELECTION] {
            let mut canvas = Canvas::new(14, 1);
            let label = "a\u{1b}界e\u{301} 👩‍💻 tail";
            canvas.draw_text(2, 0, 8, label, FOREGROUND, background);
            canvas.highlight_matches(2, 0, 8, label, &[2, 3, 5, 9]);
            for column in [3, 4, 5, 7, 8] {
                assert_eq!(canvas.cells[column].foreground, SEARCH_MATCH);
                assert!(canvas.cells[column].bold);
                assert_eq!(canvas.cells[column].background, background);
            }
            assert_eq!(canvas.cells[2].foreground, FOREGROUND);
            assert_eq!(canvas.cells[6].foreground, FOREGROUND);
            assert_eq!(canvas.cells[10], RenderCell::default());
            assert_eq!(canvas.cells[5].text, "e\u{301}");
            assert!(canvas.cells[4].wide_continuation);
            assert!(canvas.cells[8].wide_continuation);
            canvas.draw_text(2, 0, 8, label, FOREGROUND, background);
            canvas.highlight_matches(2, 0, 8, label, &[]);
            assert!(!canvas.cells.iter().any(|cell| cell.bold));
        }
        let mut canvas = Canvas::new(4, 1);
        canvas.draw_text(0, 0, 4, "a\r\nb", FOREGROUND, BACKGROUND);
        canvas.highlight_matches(0, 0, 4, "a\r\nb", &[3]);
        assert_eq!(canvas.cells[1].text, "b");
        assert!(canvas.cells[1].bold);
    }

    #[test]
    fn overlay_text_uses_grapheme_width_and_filters_controls() {
        let mut canvas = Canvas::new(8, 1);
        canvas.draw_text(1, 0, 5, "界e\u{301}\u{1b}\n\t界!", FOREGROUND, BACKGROUND);
        assert_eq!(canvas.cells[1].text, "界");
        assert!(canvas.cells[2].wide_continuation);
        assert_eq!(canvas.cells[3].text, "e\u{301}");
        assert_eq!(canvas.cells[4].text, "界");
        assert!(canvas.cells[5].wide_continuation);
        assert_eq!(canvas.cells[6], RenderCell::default());
        assert_eq!(clipped_text("界", 1), "");
        assert_eq!(clipped_text("👩‍💻!", 2), "👩‍💻");
        assert_eq!(clipped_text("\u{301}A", 2), "A");
        canvas.draw_text(7, 0, 100, "界", FOREGROUND, BACKGROUND);
        assert_eq!(canvas.cells[7].text, " ");
        canvas.draw_text(1, 0, 5, "abc", FOREGROUND, BACKGROUND);
        assert!(!canvas.cells.iter().any(|cell| cell.wide_continuation));
    }

    #[test]
    fn parses_configured_truecolor_borders() {
        assert_eq!(
            parse_color("#3971ED").unwrap(),
            Color::Rgb {
                r: 0x39,
                g: 0x71,
                b: 0xed
            }
        );
        assert!(parse_color("blue").is_err());
    }

    #[test]
    fn split_indicator_highlights_the_edge_where_the_new_pane_will_open() {
        let rect = Rect::new(2, 1, 5, 4);
        let mut horizontal = Canvas::new(10, 8);
        horizontal.draw_border(rect, FOCUSED_BORDER);
        horizontal.draw_split_indicator(rect, Orientation::Horizontal);
        let right_edge = usize::from(2_u16 + 5 - 1) + usize::from(10_u16) * usize::from(2_u16);
        assert_eq!(horizontal.cells[right_edge].foreground, SPLIT_INDICATOR);
        assert_eq!(horizontal.cells[right_edge].text, "│");
        assert_eq!(horizontal.cells[16].foreground, FOCUSED_BORDER);
        assert_eq!(horizontal.cells[16].text, "┐");
        assert_eq!(horizontal.cells[46].foreground, FOCUSED_BORDER);
        assert_eq!(horizontal.cells[46].text, "┘");

        let mut vertical = Canvas::new(10, 8);
        vertical.draw_border(rect, FOCUSED_BORDER);
        vertical.draw_split_indicator(rect, Orientation::Vertical);
        let bottom_edge = usize::from(3_u16) + usize::from(10_u16) * usize::from(1_u16 + 4 - 1);
        assert_eq!(vertical.cells[bottom_edge].foreground, SPLIT_INDICATOR);
        assert_eq!(vertical.cells[bottom_edge].text, "─");
        assert_eq!(vertical.cells[42].foreground, FOCUSED_BORDER);
        assert_eq!(vertical.cells[42].text, "└");
        assert_eq!(vertical.cells[46].foreground, FOCUSED_BORDER);
        assert_eq!(vertical.cells[46].text, "┘");
    }

    #[test]
    fn workspace_bar_draws_only_open_workspaces_and_highlights_active() {
        let mut canvas = Canvas::new(30, 4);
        let mut open = [false; 10];
        open[0] = true;
        open[2] = true;
        canvas.draw_workspace_bar(
            Rect::new(0, 0, 30, 4),
            WorkspaceBar { active: 2, open },
            FOCUSED_BORDER,
        );

        assert_eq!(canvas.cells[1].text, "1");
        assert_eq!(canvas.cells[1].background, FOCUSED_BORDER);
        assert_eq!(canvas.cells[4].text, "3");
        assert_eq!(canvas.cells[4].background, SELECTION);
        assert!(!canvas.cells.iter().any(|cell| cell.text == "2"));
    }
}
