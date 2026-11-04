//! Where the badge sits relative to the usage widget. Pure geometry.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Side {
    /// The side of the widget with more free taskbar.
    #[default]
    Auto,
    /// Left of (or above, on a vertical taskbar) the widget.
    Before,
    /// Right of (or below) the widget.
    After,
}

impl Side {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "left" | "izquierda" | "before" | "top" | "arriba" => Self::Before,
            "right" | "derecha" | "after" | "bottom" | "abajo" => Self::After,
            _ => Self::Auto,
        }
    }
}

/// Square badge next to `anchor` (the widget), inside `bar` (its taskbar)
/// when known. `scale` is a percentage of the widget's thickness.
pub fn badge_rect(anchor: Rect, bar: Option<Rect>, side: Side, scale_percent: u32) -> Rect {
    let vertical_bar = bar.is_some_and(|bar| bar.height() > bar.width());
    let thickness = if vertical_bar { anchor.width() } else { anchor.height() };
    let size = ((thickness.max(1) as i64 * scale_percent.clamp(50, 150) as i64 / 100) as i32).clamp(20, 112);
    let gap = (size / 16).max(2);

    if vertical_bar {
        let bar = bar.unwrap_or(anchor);
        let room_before = anchor.top - bar.top;
        let room_after = bar.bottom - anchor.bottom;
        let before = match side {
            Side::Before => true,
            Side::After => false,
            Side::Auto => room_before > room_after,
        };
        let left = anchor.left + (anchor.width() - size) / 2;
        let top = if before { anchor.top - gap - size } else { anchor.bottom + gap };
        return Rect { left, top, right: left + size, bottom: top + size };
    }

    let before = match side {
        Side::Before => true,
        Side::After => false,
        Side::Auto => match bar {
            Some(bar) => anchor.left - bar.left > bar.right - anchor.right,
            None => true,
        },
    };
    let top = anchor.top + (anchor.height() - size) / 2;
    let left = if before { anchor.left - gap - size } else { anchor.right + gap };
    Rect { left, top, right: left + size, bottom: top + size }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAR: Rect = Rect { left: 0, top: 1032, right: 1920, bottom: 1080 };

    #[test]
    fn auto_uses_the_roomier_side() {
        // Widget next to the tray: badge goes to its left.
        let tray_side = Rect { left: 1500, top: 1033, right: 1700, bottom: 1079 };
        let badge = badge_rect(tray_side, Some(BAR), Side::Auto, 100);
        assert!(badge.right < tray_side.left);
        assert_eq!(badge.width(), 46);
        assert_eq!(badge.height(), 46);
        // Widget at the free left end of a centred taskbar: badge to its right.
        let left_end = Rect { left: 8, top: 1033, right: 208, bottom: 1079 };
        let badge = badge_rect(left_end, Some(BAR), Side::Auto, 100);
        assert!(badge.left > left_end.right);
        assert_eq!(badge.top, 1033);
    }

    #[test]
    fn vertical_taskbars_stack_the_badge() {
        let bar = Rect { left: 0, top: 0, right: 62, bottom: 1080 };
        let widget = Rect { left: 2, top: 900, right: 60, bottom: 1000 };
        let badge = badge_rect(widget, Some(bar), Side::Auto, 100);
        assert!(badge.bottom < widget.top);
        assert_eq!(badge.width(), 58);
    }

    #[test]
    fn size_is_clamped_and_side_can_be_forced() {
        let tiny = Rect { left: 100, top: 0, right: 200, bottom: 4 };
        let badge = badge_rect(tiny, None, Side::After, 100);
        assert_eq!(badge.width(), 20);
        assert!(badge.left > tiny.right);
        assert_eq!(Side::parse("Derecha"), Side::After);
        assert_eq!(Side::parse("whatever"), Side::Auto);
    }
}
