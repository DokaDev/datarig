//! Drawing: line numbers, the bar of what a run takes, syntax colors of the lines on screen
//! (lexed from the cached line states), the matches of a search on them, the selection, the
//! cursor line, and the search prompt on the last line while it is open.

use super::buffer::gw;
use super::{Editor, Mode};
use crate::theme;
use datarig_core::sql::lexer::{Tok, lex};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_segmentation::UnicodeSegmentation;

impl Editor {
    /// Screen position of the cursor inside `area` if it were rendered now. `stmt` is the byte
    /// range of the statement a run would take: its lines get a faint tint and a bar in the
    /// gutter. With a Visual selection the bar marks the selection's lines instead (a run takes
    /// the selection), and `stmt` is not used. While the search prompt is open it takes the
    /// last line, and the cursor is in it.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer, stmt: Option<(usize, usize)>) -> (u16, u16) {
        let th = theme::cur();
        if self.prompt.is_some() && area.height == 1 {
            // No room for the text: the prompt takes the only line.
            return self.render_prompt(area, buf, area.y);
        }
        let prompt_h = u16::from(self.prompt.is_some());
        let h = (area.height - prompt_h) as usize;
        let numw = self.lines.len().to_string().len().max(3);
        self.gutter = numw + 1;
        self.view_h = h;
        let text_w = (area.width as usize).saturating_sub(self.gutter).max(1);
        if self.row < self.top {
            self.top = self.row;
        } else if self.row >= self.top + h {
            self.top = self.row + 1 - h;
        }
        let cx = self.display_x(self.row, self.col);
        if cx < self.left {
            self.left = cx;
        } else if cx >= self.left + text_w {
            self.left = cx + 1 - text_w;
        }

        // Tokens of the lines on screen, lexed from where the lexer's state is known.
        let last = (self.top + h).min(self.lines.len());
        let (base, region) = self.region_text(self.top, last);
        let toks = lex(&region);
        let is_fn: Vec<bool> = toks
            .iter()
            .enumerate()
            .map(|(i, t)| t.kind == Tok::Ident && toks.get(i + 1).is_some_and(|n| n.kind == Tok::LParen))
            .collect();
        let sel = (self.mode == Mode::Visual).then(|| self.visual_bounds());
        let stmt = if sel.is_some() { None } else { stmt };
        // What a run takes: the selection, else the statement under the cursor.
        let run = sel.or(stmt);

        // The matches of the search on the lines on screen, found line by line as they are drawn.
        let hl = self.highlight_re();
        let mut highlighted = 0;

        let mut ti = 0;
        let mut line_start = self.line_start(self.top.min(self.lines.len()));
        // Where line `top` starts in `region`.
        let mut rstart = line_start - base;
        for vy in 0..h {
            let r = self.top + vy;
            let y = area.y + vy as u16;
            if r >= self.lines.len() {
                buf.set_style(Rect::new(area.x, y, area.width, 1), th.base());
                continue;
            }
            let line = &self.lines[r];
            let line_end = line_start + line.len();
            let in_stmt = stmt.is_some_and(|(a, b)| line_start < b && a <= line_end);
            let bg = if r == self.row && self.mode != Mode::Visual {
                th.cursor_line
            } else if in_stmt {
                th.current_stmt
            } else {
                Style::new().bg(th.bg)
            };
            buf.set_style(Rect::new(area.x, y, area.width, 1), Style::new().fg(th.fg).patch(bg));
            let num_fg = if r == self.row { th.fg } else { th.fg_muted };
            let num = format!("{:>numw$} ", r + 1);
            buf.set_stringn(area.x, y, &num, self.gutter, Style::new().fg(num_fg).patch(bg));
            if run.is_some_and(|(a, b)| line_start < b && a <= line_end) {
                // The bar sits in the blank between the line number and the text.
                let style = Style::new().fg(th.current_stmt_bar).patch(bg);
                buf.set_stringn(area.x + numw as u16, y, "▎", 1, style);
            }

            let tx0 = area.x + self.gutter as u16;
            let mut matches = hl.map(|re| re.find_iter(line).filter(|m| !m.is_empty()).peekable());
            highlighted += usize::from(hl.is_some());
            let mut x = 0usize;
            let mut b = line_start;
            let mut rb = rstart;
            for g in line.graphemes(true) {
                let w = gw(g);
                while ti < toks.len() && toks[ti].end <= rb {
                    ti += 1;
                }
                let mut style = match toks.get(ti) {
                    Some(t) if t.start <= rb => th.syntax(t.kind, is_fn[ti]),
                    _ => Style::new().fg(th.fg),
                }
                .patch(bg);
                if let Some(ms) = matches.as_mut() {
                    let lb = b - line_start;
                    // A match on any byte of the grapheme marks it (one may start inside it).
                    while ms.next_if(|m| m.end() <= lb).is_some() {}
                    if ms.peek().is_some_and(|m| m.start() < lb + g.len()) {
                        style = style.patch(th.search_match);
                    }
                }
                if sel.is_some_and(|(a, z)| b >= a && b < z) {
                    style = style.patch(th.selection);
                }
                if x + w > self.left && x < self.left + text_w {
                    if x < self.left || x + w > self.left + text_w {
                        // partially visible wide grapheme: blank the visible part
                        let from = x.max(self.left);
                        let to = (x + w).min(self.left + text_w);
                        for px in from..to {
                            buf.set_stringn(tx0 + (px - self.left) as u16, y, " ", 1, style);
                        }
                    } else if g == "\t" {
                        buf.set_stringn(tx0 + (x - self.left) as u16, y, "    ", w, style);
                    } else {
                        buf.set_stringn(tx0 + (x - self.left) as u16, y, g, w, style);
                    }
                }
                x += w;
                b += g.len();
                rb += g.len();
            }
            if let Some((a, z)) = sel {
                // show selected line breaks as a one-cell highlight
                if line_end >= a && line_end < z && x >= self.left && x < self.left + text_w {
                    buf.set_stringn(tx0 + (x - self.left) as u16, y, " ", 1, th.selection);
                }
            }
            line_start = line_end + 1;
            rstart += line.len() + 1;
        }
        self.search_work.highlighted += highlighted;
        let cy = area.y + (self.row - self.top) as u16;
        let cxs = area.x + self.gutter as u16 + (cx - self.left) as u16;
        if prompt_h > 0 {
            return self.render_prompt(area, buf, area.y + h as u16);
        }
        (cxs.min(area.x + area.width.saturating_sub(1)), cy)
    }

    /// The search prompt on line `y` of `area`; where the cursor is in it.
    fn render_prompt(&mut self, area: Rect, buf: &mut Buffer, y: u16) -> (u16, u16) {
        let th = theme::cur();
        let Some(p) = self.prompt.as_mut() else { return (area.x, y) };
        let style = Style::new().fg(th.fg).bg(th.bg);
        buf.set_stringn(area.x, y, if p.forward { "/" } else { "?" }, 1, style);
        let input = Rect::new(area.x + 1, y, area.width.saturating_sub(1), 1);
        (p.input.render(input, buf, style, true, false, None), y)
    }
}
