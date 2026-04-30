use std::num::NonZeroUsize;

#[derive(Debug, Copy, Clone)]
pub struct Dimensions {
    pub width: usize,
    pub height: usize,
}

pub const DEFAULT_WIDTH: NonZeroUsize = unsafe { NonZeroUsize::new_unchecked(80) };
pub const DEFAULT_HEIGHT: NonZeroUsize = unsafe { NonZeroUsize::new_unchecked(24) };

impl Default for Dimensions {
    fn default() -> Self {
        Dimensions {
            width: DEFAULT_WIDTH.get(),
            height: DEFAULT_HEIGHT.get(),
        }
    }
}

pub fn current() -> Dimensions {
    let Ok((columns, rows)) = termion::terminal_size() else {
        panic!("Unable to get terminal size")
    };

    Dimensions {
        width: columns as usize,
        height: rows as usize,
    }
}
