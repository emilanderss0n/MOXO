// The image being edited: its pixels, the file it belongs to, and its undo
// history. Nothing here knows about GPUI or windows, so it's all testable
// without opening one. Zoom and pan live in `viewport.rs`, not here.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::{RgbaImage, imageops};

use crate::save::FileFormat;

// The edits Moxo can make so far. All of them are exact and reversible, so
// undo applies the opposite transform instead of keeping a copy of the image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transform {
    RotateClockwise,
    RotateCounterClockwise,
    FlipHorizontal,
    FlipVertical,
}

impl Transform {
    fn inverse(self) -> Self {
        match self {
            Self::RotateClockwise => Self::RotateCounterClockwise,
            Self::RotateCounterClockwise => Self::RotateClockwise,
            flip => flip,
        }
    }

    // Rotations swap width and height, so the view has to be refitted.
    pub fn changes_size(self) -> bool {
        matches!(self, Self::RotateClockwise | Self::RotateCounterClockwise)
    }

    fn apply(self, pixels: &mut Arc<RgbaImage>) {
        match self {
            Self::RotateClockwise => *pixels = Arc::new(imageops::rotate90(&**pixels)),
            Self::RotateCounterClockwise => *pixels = Arc::new(imageops::rotate270(&**pixels)),
            // `make_mut` edits in place, unless a save in progress is still
            // reading these pixels; then it edits a copy and the save keeps the
            // original.
            Self::FlipHorizontal => imageops::flip_horizontal_in_place(Arc::make_mut(pixels)),
            Self::FlipVertical => imageops::flip_vertical_in_place(Arc::make_mut(pixels)),
        }
    }
}

// Things the file on disk holds that Moxo can't keep, detected while loading.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceDetails {
    // More than 8 bits per channel (a 16-bit PNG); Moxo works in 8 bits.
    pub high_bit_depth: bool,
    // An embedded colour profile; Moxo doesn't write one.
    pub colour_profile: bool,
}

// Information a save would lose, as far as Moxo can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loss {
    BitDepth,
    ColourProfile,
    Transparency,
}

impl Loss {
    pub fn describe(self) -> &'static str {
        match self {
            Self::BitDepth => "The image will be reduced from 16 to 8 bits per channel.",
            Self::ColourProfile => "Its colour profile will be removed.",
            Self::Transparency => {
                "JPEG can't store transparency, so transparent areas will become white."
            }
        }
    }
}

// One undoable step. The state numbers say which version of the image the
// document is in before and after it.
#[derive(Clone, Copy, Debug)]
struct Step {
    transform: Transform,
    state_before: u64,
    state_after: u64,
}

// What a background save writes: the pixels as they were when the save started,
// and which version of the document that was.
pub struct SaveSnapshot {
    pub pixels: Arc<RgbaImage>,
    pub state: u64,
}

pub struct Document {
    pixels: Arc<RgbaImage>,
    path: PathBuf,
    format: FileFormat,
    source: SourceDetails,
    // Every edit, undo and redo moves to a numbered state; the document has
    // unsaved changes whenever it isn't in the state that was last saved. So
    // undoing back to the saved version counts as unchanged.
    state: u64,
    saved_state: u64,
    next_state: u64,
    undo: Vec<Step>,
    redo: Vec<Step>,
}

impl Document {
    pub fn new(
        pixels: RgbaImage,
        path: PathBuf,
        format: FileFormat,
        source: SourceDetails,
    ) -> Self {
        Self {
            pixels: Arc::new(pixels),
            path,
            format,
            source,
            state: 0,
            saved_state: 0,
            next_state: 1,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn pixels(&self) -> &RgbaImage {
        &self.pixels
    }

    pub fn width(&self) -> u32 {
        self.pixels.width()
    }

    pub fn height(&self) -> u32 {
        self.pixels.height()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn format(&self) -> FileFormat {
        self.format
    }

    pub fn has_unsaved_changes(&self) -> bool {
        self.state != self.saved_state
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn apply(&mut self, transform: Transform) {
        transform.apply(&mut self.pixels);
        let state_before = self.state;
        self.state = self.next_state;
        self.next_state += 1;
        self.undo.push(Step {
            transform,
            state_before,
            state_after: self.state,
        });
        // A new edit starts a new branch; the undone steps can't come back.
        self.redo.clear();
    }

    // Undoes the last edit. Returns the transform that was applied to the
    // pixels, or `None` if there was nothing to undo.
    pub fn undo(&mut self) -> Option<Transform> {
        let step = self.undo.pop()?;
        let reverse = step.transform.inverse();
        reverse.apply(&mut self.pixels);
        self.state = step.state_before;
        self.redo.push(step);
        Some(reverse)
    }

    pub fn redo(&mut self) -> Option<Transform> {
        let step = self.redo.pop()?;
        step.transform.apply(&mut self.pixels);
        self.state = step.state_after;
        self.undo.push(step);
        Some(step.transform)
    }

    // Shares the current pixels with a save running in the background. Cheap:
    // the pixels are only copied if the document is edited before it finishes.
    pub fn save_snapshot(&self) -> SaveSnapshot {
        SaveSnapshot {
            pixels: Arc::clone(&self.pixels),
            state: self.state,
        }
    }

    // Records a finished save of the version numbered `saved_state`. If the
    // image was edited while it was saving, it stays marked as unsaved.
    pub fn mark_saved(&mut self, saved_state: u64, path: PathBuf, format: FileFormat) {
        self.saved_state = saved_state;
        self.path = path;
        self.format = format;
        // The file now on disk was written by Moxo, so it holds nothing more
        // than the document does.
        self.source = SourceDetails::default();
    }

    // What saving to `path` as `format` would lose, as far as Moxo can detect.
    // The 16-bit and colour-profile details describe the document's own file,
    // so they only matter when overwriting it; Save As elsewhere leaves that
    // file untouched.
    pub fn losses_when_saving(&self, path: &Path, format: FileFormat) -> Vec<Loss> {
        let mut losses = Vec::new();
        if is_same_file(path, &self.path) {
            if self.source.high_bit_depth {
                losses.push(Loss::BitDepth);
            }
            if self.source.colour_profile {
                losses.push(Loss::ColourProfile);
            }
        }
        if format == FileFormat::Jpeg && self.pixels.pixels().any(|pixel| pixel[3] < 255) {
            losses.push(Loss::Transparency);
        }
        losses
    }
}

// Compares the real files when both exist, so different spellings of the same
// path (letter case, `.\`) still match on Windows.
fn is_same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    // A 3 × 2 image where every pixel is different, including transparent ones:
    //   a b c
    //   d e f
    fn sample() -> RgbaImage {
        let mut pixels = RgbaImage::new(3, 2);
        let values = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 128],
            [10, 20, 30, 0],
            [40, 50, 60, 255],
            [70, 80, 90, 1],
        ];
        for (index, value) in values.into_iter().enumerate() {
            pixels.put_pixel(index as u32 % 3, index as u32 / 3, Rgba(value));
        }
        pixels
    }

    fn document() -> Document {
        Document::new(
            sample(),
            PathBuf::from("photo.png"),
            FileFormat::Png,
            SourceDetails::default(),
        )
    }

    // Pixel (x, y) of the original sample.
    fn original(x: u32, y: u32) -> [u8; 4] {
        sample().get_pixel(x, y).0
    }

    fn pixel(document: &Document, x: u32, y: u32) -> [u8; 4] {
        document.pixels().get_pixel(x, y).0
    }

    #[test]
    fn a_new_document_is_clean_with_its_path_and_format() {
        let document = document();
        assert!(!document.has_unsaved_changes());
        assert!(!document.can_undo());
        assert!(!document.can_redo());
        assert_eq!(document.path(), Path::new("photo.png"));
        assert_eq!(document.file_name(), "photo.png");
        assert_eq!(document.format(), FileFormat::Png);
        assert_eq!((document.width(), document.height()), (3, 2));
        assert_eq!(*document.pixels(), sample());
    }

    #[test]
    fn rotating_clockwise_moves_every_pixel_exactly() {
        // a b c        d a
        // d e f   ->   e b
        //              f c
        let mut document = document();
        document.apply(Transform::RotateClockwise);
        assert_eq!((document.width(), document.height()), (2, 3));
        let expected = [
            ((0, 0), (0, 1)),
            ((1, 0), (0, 0)),
            ((0, 1), (1, 1)),
            ((1, 1), (1, 0)),
            ((0, 2), (2, 1)),
            ((1, 2), (2, 0)),
        ];
        for ((x, y), (from_x, from_y)) in expected {
            assert_eq!(pixel(&document, x, y), original(from_x, from_y));
        }
    }

    #[test]
    fn rotating_counter_clockwise_moves_every_pixel_exactly() {
        // a b c        c f
        // d e f   ->   b e
        //              a d
        let mut document = document();
        document.apply(Transform::RotateCounterClockwise);
        assert_eq!((document.width(), document.height()), (2, 3));
        let expected = [
            ((0, 0), (2, 0)),
            ((1, 0), (2, 1)),
            ((0, 1), (1, 0)),
            ((1, 1), (1, 1)),
            ((0, 2), (0, 0)),
            ((1, 2), (0, 1)),
        ];
        for ((x, y), (from_x, from_y)) in expected {
            assert_eq!(pixel(&document, x, y), original(from_x, from_y));
        }
    }

    #[test]
    fn flips_mirror_every_pixel_and_keep_the_size() {
        let mut horizontal = document();
        horizontal.apply(Transform::FlipHorizontal);
        let mut vertical = document();
        vertical.apply(Transform::FlipVertical);
        assert_eq!((horizontal.width(), horizontal.height()), (3, 2));
        assert_eq!((vertical.width(), vertical.height()), (3, 2));
        for y in 0..2 {
            for x in 0..3 {
                assert_eq!(pixel(&horizontal, x, y), original(2 - x, y));
                assert_eq!(pixel(&vertical, x, y), original(x, 1 - y));
            }
        }
    }

    #[test]
    fn transforms_keep_transparency_exactly() {
        for transform in [
            Transform::RotateClockwise,
            Transform::RotateCounterClockwise,
            Transform::FlipHorizontal,
            Transform::FlipVertical,
        ] {
            let mut document = document();
            document.apply(transform);
            let mut alphas: Vec<u8> = document.pixels().pixels().map(|p| p[3]).collect();
            alphas.sort();
            assert_eq!(alphas, [0, 1, 128, 255, 255, 255], "{transform:?}");
        }
    }

    #[test]
    fn transforms_that_cancel_out_restore_the_exact_original() {
        let mut document = document();
        for _ in 0..4 {
            document.apply(Transform::RotateClockwise);
        }
        assert_eq!(*document.pixels(), sample());

        document.apply(Transform::RotateClockwise);
        document.apply(Transform::RotateCounterClockwise);
        document.apply(Transform::FlipHorizontal);
        document.apply(Transform::FlipHorizontal);
        document.apply(Transform::FlipVertical);
        document.apply(Transform::FlipVertical);
        assert_eq!(*document.pixels(), sample());
    }

    #[test]
    fn an_edit_marks_the_document_unsaved_and_undo_returns_to_saved() {
        let mut document = document();
        document.apply(Transform::FlipVertical);
        assert!(document.has_unsaved_changes());
        assert!(document.can_undo());

        assert_eq!(document.undo(), Some(Transform::FlipVertical));
        assert!(!document.has_unsaved_changes());
        assert_eq!(*document.pixels(), sample());
    }

    #[test]
    fn undo_and_redo_walk_through_a_sequence_exactly() {
        let mut document = document();
        document.apply(Transform::RotateClockwise);
        let after_rotate = document.pixels().clone();
        document.apply(Transform::FlipHorizontal);
        let after_flip = document.pixels().clone();

        // Undoing a rotation applies the opposite rotation, so the size changes back.
        assert_eq!(document.undo(), Some(Transform::FlipHorizontal));
        assert_eq!(*document.pixels(), after_rotate);
        assert_eq!(document.undo(), Some(Transform::RotateCounterClockwise));
        assert_eq!(*document.pixels(), sample());
        assert!(!document.can_undo());
        assert_eq!(document.undo(), None);
        assert_eq!(*document.pixels(), sample());

        assert_eq!(document.redo(), Some(Transform::RotateClockwise));
        assert_eq!(*document.pixels(), after_rotate);
        assert_eq!(document.redo(), Some(Transform::FlipHorizontal));
        assert_eq!(*document.pixels(), after_flip);
        assert!(!document.can_redo());
        assert_eq!(document.redo(), None);
        assert_eq!(*document.pixels(), after_flip);
    }

    #[test]
    fn redo_after_returning_to_the_saved_state_is_unsaved_again() {
        let mut document = document();
        document.apply(Transform::FlipHorizontal);
        document.undo();
        assert!(!document.has_unsaved_changes());
        document.redo();
        assert!(document.has_unsaved_changes());
    }

    #[test]
    fn a_new_edit_after_undo_clears_redo() {
        let mut document = document();
        document.apply(Transform::FlipHorizontal);
        document.undo();
        assert!(document.can_redo());
        document.apply(Transform::FlipVertical);
        assert!(!document.can_redo());
        assert_eq!(document.redo(), None);
    }

    #[test]
    fn a_different_edit_back_to_identical_pixels_still_counts_as_unsaved() {
        // Flipping twice gives the original pixels, but through two new edits,
        // not undo, so it's a different version of the document.
        let mut document = document();
        document.apply(Transform::FlipHorizontal);
        document.apply(Transform::FlipHorizontal);
        assert_eq!(*document.pixels(), sample());
        assert!(document.has_unsaved_changes());
    }

    #[test]
    fn saving_marks_the_saved_version_and_records_the_new_file() {
        let mut document = document();
        document.apply(Transform::RotateClockwise);
        let snapshot = document.save_snapshot();
        document.mark_saved(snapshot.state, PathBuf::from("copy.jpg"), FileFormat::Jpeg);

        assert!(!document.has_unsaved_changes());
        assert_eq!(document.path(), Path::new("copy.jpg"));
        assert_eq!(document.file_name(), "copy.jpg");
        assert_eq!(document.format(), FileFormat::Jpeg);
        // Undo still works after saving, and leaves the saved version.
        document.undo();
        assert!(document.has_unsaved_changes());
    }

    // The background-save race: the user edits while an older version is
    // being written. When that save finishes, the newer version must still
    // count as unsaved.
    #[test]
    fn an_edit_made_during_a_save_stays_unsaved_when_the_save_finishes() {
        let mut document = document();
        document.apply(Transform::FlipHorizontal);
        let snapshot = document.save_snapshot();

        document.apply(Transform::FlipVertical);
        document.mark_saved(snapshot.state, PathBuf::from("photo.png"), FileFormat::Png);
        assert!(document.has_unsaved_changes());

        // Undoing back to exactly the version that was written is clean.
        document.undo();
        assert!(!document.has_unsaved_changes());
    }

    #[test]
    fn a_save_snapshot_keeps_its_pixels_when_the_document_is_edited() {
        let mut document = document();
        let snapshot = document.save_snapshot();
        // Flips edit in place, so this checks the copy-on-write: the save in
        // progress must still see the pixels it started with.
        document.apply(Transform::FlipHorizontal);
        document.apply(Transform::RotateClockwise);
        assert_eq!(*snapshot.pixels, sample());
        assert_ne!(*document.pixels(), sample());
    }

    fn document_from(source: SourceDetails, path: &str, opaque: bool) -> Document {
        let mut pixels = RgbaImage::from_pixel(2, 2, Rgba([1, 2, 3, 255]));
        if !opaque {
            pixels.put_pixel(1, 1, Rgba([1, 2, 3, 254]));
        }
        Document::new(pixels, PathBuf::from(path), FileFormat::Png, source)
    }

    #[test]
    fn overwriting_a_16_bit_or_profiled_file_reports_what_is_lost() {
        let source = SourceDetails {
            high_bit_depth: true,
            colour_profile: true,
        };
        let document = document_from(source, "deep.png", true);
        assert_eq!(
            document.losses_when_saving(Path::new("deep.png"), FileFormat::Png),
            [Loss::BitDepth, Loss::ColourProfile]
        );
        // Save As elsewhere leaves the original file alone, so nothing is lost from it.
        assert_eq!(
            document.losses_when_saving(Path::new("other.png"), FileFormat::Png),
            []
        );
    }

    #[test]
    fn saving_transparency_as_jpeg_is_reported_but_opaque_images_are_fine() {
        let see_through = document_from(SourceDetails::default(), "a.png", false);
        assert_eq!(
            see_through.losses_when_saving(Path::new("a.jpg"), FileFormat::Jpeg),
            [Loss::Transparency]
        );
        assert_eq!(
            see_through.losses_when_saving(Path::new("a.png"), FileFormat::Png),
            []
        );

        let opaque = document_from(SourceDetails::default(), "b.png", true);
        assert_eq!(
            opaque.losses_when_saving(Path::new("b.jpg"), FileFormat::Jpeg),
            []
        );
    }

    #[test]
    fn after_saving_the_original_file_details_no_longer_apply() {
        let source = SourceDetails {
            high_bit_depth: true,
            colour_profile: false,
        };
        let mut document = document_from(source, "deep.png", true);
        let state = document.save_snapshot().state;
        document.mark_saved(state, PathBuf::from("deep.png"), FileFormat::Png);
        // The file is now Moxo's own 8-bit file, so saving again loses nothing.
        assert_eq!(
            document.losses_when_saving(Path::new("deep.png"), FileFormat::Png),
            []
        );
    }
}
