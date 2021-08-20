pub const DEFAULT_WIDTH: u16 = 80;
pub const DEFAULT_HEIGHT: u16 = 24;
pub const STATUS_BAR_HEIGHT: u16 = 2;

#[derive(Copy, Clone, Debug)]
pub struct TTYDimensions {
    pub width: u16,
    pub height: u16,
}

impl TTYDimensions {
    pub fn from_size(size: (u16, u16)) -> TTYDimensions {
        TTYDimensions {
            width: size.0,
            height: size.1,
        }
    }

    pub fn without_status_bar(&self) -> TTYDimensions {
        TTYDimensions {
            width: self.width,
            height: if self.height < STATUS_BAR_HEIGHT {
                0
            } else {
                self.height - STATUS_BAR_HEIGHT
            },
        }
    }
}

impl Default for TTYDimensions {
    fn default() -> TTYDimensions {
        TTYDimensions {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
        }
    }
}
