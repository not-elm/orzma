//! The shelf packer that places each sprite on a row of the atlas.

/// The packing cursor of an atlas: sprites go left to right along the
/// current shelf, and one that does not fit starts a new shelf below it.
pub struct Shelves {
    width: u32,
    height: u32,
    y: u32,
    shelf: Shelf,
}

impl Shelves {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            y: 0,
            shelf: Shelf::default(),
        }
    }

    #[inline]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[inline]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// The left column, in atlas pixels, where the next sprite goes.
    #[inline]
    pub const fn x(&self) -> u32 {
        self.shelf.x
    }

    /// The top row, in atlas pixels, of the current shelf.
    #[inline]
    pub const fn y(&self) -> u32 {
        self.y
    }

    #[inline]
    pub fn new_line_if_need(&mut self, font_width: u16) {
        if self.width < self.shelf.x + font_width as u32 {
            self.shelf.x = 0;
            self.y = self.y.saturating_add(self.shelf.height);
            self.shelf.height = 0;
        }
    }

    /// Whether the current shelf, grown to at least `height` pixels,
    /// would not fit below the current row.
    #[inline]
    pub fn would_overflow(&self, height: u16) -> bool {
        self.height < self.y + self.shelf.height.max(u32::from(height))
    }

    #[inline]
    pub fn clear(&mut self) {
        self.shelf.x = 0;
        self.y = 0;
        self.shelf.height = 0;
    }

    #[inline]
    pub fn advance_x(&mut self, font_width: u16) {
        self.shelf.x = self.shelf.x.saturating_add(font_width as u32);
    }

    #[inline]
    pub fn adjust_shelf_height(&mut self, font_height: u16) {
        self.shelf.height = self.shelf.height.max(font_height as u32);
    }
}

#[derive(Default)]
struct Shelf {
    height: u32,
    x: u32,
}
