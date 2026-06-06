use alacritty_terminal::{index::Point, term::TermMode};

#[derive(Clone, Copy)]
pub enum MouseReportKind {
    Press(u8),
    Release(u8),
    Move(u8),
    ScrollUp,
    ScrollDown,
}

#[derive(Clone, Copy)]
pub struct MouseReport {
    pub point: Point,
    pub kind: MouseReportKind,
}

pub fn encode_mouse_report(
    report: MouseReport,
    mode: TermMode,
    display_offset: usize,
) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }

    if matches!(report.kind, MouseReportKind::Move(_))
        && !mode.contains(TermMode::MOUSE_MOTION)
        && !mode.contains(TermMode::MOUSE_DRAG)
    {
        return None;
    }

    let mut code = match report.kind {
        MouseReportKind::Press(button) => button.min(2),
        MouseReportKind::Release(button) => {
            if mode.contains(TermMode::SGR_MOUSE) {
                button.min(2)
            } else {
                3
            }
        }
        MouseReportKind::Move(button) => (if button <= 2 { button } else { 3 }) | 32,
        MouseReportKind::ScrollUp => 64,
        MouseReportKind::ScrollDown => 65,
    };

    let col = report.point.column.0.saturating_add(1);
    let row = (report.point.line.0 + display_offset as i32)
        .max(0)
        .saturating_add(1) as usize;

    if mode.contains(TermMode::SGR_MOUSE) {
        let suffix = if matches!(report.kind, MouseReportKind::Release(_)) {
            'm'
        } else {
            'M'
        };
        Some(format!("\x1b[<{};{};{}{}", code, col, row, suffix).into_bytes())
    } else if mode.contains(TermMode::UTF8_MOUSE) {
        code = code.saturating_add(32);
        Some(
            format!(
                "\x1b[M{}{}{}",
                code as char,
                encode_utf8_mouse_coord(col),
                encode_utf8_mouse_coord(row)
            )
            .into_bytes(),
        )
    } else {
        code = code.saturating_add(32);
        Some(vec![
            0x1b,
            b'[',
            b'M',
            code,
            (col + 32) as u8,
            (row + 32) as u8,
        ])
    }
}

fn encode_utf8_mouse_coord(value: usize) -> char {
    char::from_u32((value + 32) as u32).unwrap_or('\u{fffd}')
}
