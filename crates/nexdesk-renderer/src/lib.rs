//! Renderer boundary: RDP decoding produces a framebuffer; UI decides how to present it.
use nexdesk_core::scale::{blit_fit, Fit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplaySize {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct FrameBuffer {
    width: u32,
    height: u32,
    pixels: Vec<u32>,
}

impl FrameBuffer {
    pub fn new(width: u32, height: u32) -> Option<Self> {
        let len = (width as usize).checked_mul(height as usize)?;
        Some(Self {
            width,
            height,
            pixels: vec![0; len],
        })
    }
    pub fn replace(&mut self, width: u32, height: u32, pixels: Vec<u32>) -> bool {
        if pixels.len() != (width as usize).saturating_mul(height as usize) {
            return false;
        }
        self.width = width;
        self.height = height;
        self.pixels = pixels;
        true
    }
    pub fn size(&self) -> DisplaySize {
        DisplaySize {
            width: self.width,
            height: self.height,
        }
    }
    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }
    pub fn fit(&self, dst: DisplaySize) -> Option<Fit> {
        Fit::new(self.width, self.height, dst.width, dst.height)
    }
    pub fn blit_into(&self, dst: &mut [u32], size: DisplaySize) {
        blit_fit(
            &self.pixels,
            self.width,
            self.height,
            dst,
            size.width,
            size.height,
        );
    }
}

pub trait Renderer: Send {
    fn submit(&mut self, frame: FrameBuffer);
    fn current(&self) -> Option<&FrameBuffer>;
}
