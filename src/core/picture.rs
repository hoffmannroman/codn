//! What the app knows of a picture: its format, what is to be said of
//! it, and why there is none. The decoding is in `crate::picture`, in a
//! process of its own, and nothing of the renderer is in here.

use std::time::{Duration, Instant};

/// The formats that are shown as pictures, told by the first bytes of a
/// file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    Png,
    Jpeg,
}

impl Format {
    /// The format of a file that begins with these bytes, if it is one
    /// that is shown.
    pub fn of(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Self::Jpeg)
        } else {
            None
        }
    }

    /// As the pane names it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Png => "PNG picture",
            Self::Jpeg => "JPEG picture",
        }
    }

    /// As the process is told it.
    pub fn word(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
        }
    }

    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "png" => Some(Self::Png),
            "jpeg" => Some(Self::Jpeg),
            _ => None,
        }
    }
}

/// What is to be said of a picture that is shown, each a bit.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Notes(pub u16);

impl Notes {
    /// Some of it is transparent.
    pub const ALPHA: u16 = 1;
    /// The file ends before the picture does.
    pub const EARLY: u16 = 1 << 1;
    /// A sum over its data is wrong, or it names a color it does not have.
    pub const DAMAGED: u16 = 1 << 2;
    /// It says how its colors are meant, and that is not applied.
    pub const UNAPPLIED: u16 = 1 << 3;
    /// It holds an animation, of which the picture is shown that a
    /// program shows that knows none.
    pub const ANIMATION: u16 = 1 << 4;
    /// It was made smaller than was asked for, to stay within a limit.
    pub const SMALLER: u16 = 1 << 5;
    const ALL: u16 = (1 << 6) - 1;

    pub fn has(self, note: u16) -> bool {
        self.0 & note != 0
    }

    pub fn add(&mut self, note: u16) {
        self.0 |= note;
    }

    /// The notes in a number, if it names none that is not known.
    pub fn known(bits: u16) -> Option<Self> {
        (bits & !Self::ALL == 0).then_some(Self(bits))
    }

    /// Whether the picture is not the whole of what the file was to hold.
    pub fn partial(self) -> bool {
        self.has(Self::EARLY) || self.has(Self::DAMAGED)
    }
}

/// Why a file is not shown as a picture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Refused {
    /// The process was told something it does not take.
    Asked = 1,
    /// Was said for a part of the picture before the process cut one:
    /// it is not made any more, and kept so that the bytes of the
    /// others stay what they were.
    NotYet,
    /// It is not of the format its first bytes said.
    Format,
    /// A kind of color with a depth that the format does not have.
    Kind,
    /// A PNG as Apple's tools pack it.
    Apple,
    /// More pixels than are decoded.
    Large,
    /// More bytes than are read.
    Long,
    /// Its head or its palette cannot be read, or it holds no data;
    /// of a JPEG also a frame or a table that is missing or out of
    /// its place.
    Damaged,
    /// The memory for it was not to be had.
    Memory,
    /// A progressive JPEG, which is not shown yet.
    Progressive,
    /// A way of coding that is not shown: arithmetic, lossless,
    /// hierarchical, or more than eight bits.
    Coding,
    /// A number of colors that is not shown: CMYK, or two.
    Colors,
}

impl Refused {
    pub fn from_byte(byte: u8) -> Option<Self> {
        use Refused::*;
        [Asked, NotYet, Format, Kind, Apple, Large, Long, Damaged, Memory, Progressive, Coding, Colors]
            .into_iter()
            .find(|reason| *reason as u8 == byte)
    }

    /// As the pane says it.
    pub fn reason(self) -> &'static str {
        match self {
            Self::Asked | Self::NotYet => "It could not be decoded.",
            Self::Format => "It is not the picture its first bytes say.",
            Self::Kind => "Its kind of color and its depth do not go together.",
            Self::Apple => "It is packed as Apple's tools do it, which is not shown.",
            Self::Large => "It has too many pixels to be shown.",
            Self::Long => "It is too long to be shown.",
            Self::Damaged => "It is damaged: its head or its colors cannot be read.",
            Self::Memory => "There is not enough memory to show it.",
            Self::Progressive => "It is a progressive JPEG, which is not shown yet.",
            Self::Coding => {
                "It is coded in a way that is not shown: arithmetic, lossless, or more than 8 bits."
            }
            Self::Colors => "It has a number of colors that is not shown, as CMYK has.",
        }
    }
}

/// A picture as it came out of the process, without its pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Decoded {
    /// The size of the picture as the file has it.
    pub picture: (u32, u32),
    /// The part of it that was decoded: where it begins and its size.
    /// The whole picture, or the rectangle that was asked for, cut to
    /// lie within it.
    pub part: [u32; 4],
    /// The size the part was decoded to.
    pub size: (u32, u32),
    pub notes: Notes,
}

impl Decoded {
    /// The picture as a whole, decoded to `size`.
    #[cfg(test)]
    pub fn whole(picture: (u32, u32), size: (u32, u32), notes: Notes) -> Self {
        Self { picture, part: [0, 0, picture.0, picture.1], size, notes }
    }
}

/// What a pane shows of a picture. The pixels are not here: they are a
/// texture of the renderer, known by the number of the decoding they
/// came from.
#[derive(Debug)]
pub struct Seen {
    pub format: Format,
    /// The picture that is shown, and the number of the decoding it is
    /// of. Nothing until the first answer came, or where there is none.
    pub shown: Option<(Decoded, u64)>,
    /// Why there is no picture.
    pub why: Option<String>,
    /// The decoding that was asked for and has not answered: its number.
    pub asked: Option<u64>,
    /// The file is in another state than the one the picture was made
    /// of, or no picture was made yet: it is to be decoded.
    pub stale: bool,
    /// How long the file was when it was last decoded.
    pub length: u64,
    /// Where the picture lies in its pane, and how large.
    pub look: Look,
}

impl Seen {
    pub fn new(format: Format) -> Self {
        Self { format, shown: None, why: None, asked: None, stale: true, length: 0, look: Look::default() }
    }

    /// Whether the picture that is shown cannot show what `need` says:
    /// it does not cover it, is coarser than it asks for, or is more
    /// than twice as fine, where the texture flickers when it is drawn.
    /// One that is shown at its own size is never too fine, and one
    /// that was made smaller for a limit is not made again for more.
    pub fn misfits(&self, need: &Need) -> bool {
        let Some((decoded, _)) = self.shown else { return false };
        let [x, y, w, h] = decoded.part.map(|n| n as f32);
        let [vx, vy, vw, vh] = need.visible;
        // What is asked for is cut outward to whole pixels, so that it
        // holds what is seen to the rounding of a float. A half pixel
        // would be four of the screen at 800 %: a strip with no picture.
        let covered = vx >= x - EDGE && vy >= y - EDGE && vx + vw <= x + w + EDGE && vy + vh <= y + h + EDGE;
        if !covered {
            return true;
        }
        let (own, size) = ((decoded.part[2], decoded.part[3]) == decoded.size, decoded.size);
        let wanted = (w * need.res, h * need.res);
        let coarse = size.0 as f32 + 1.0 < wanted.0 || size.1 as f32 + 1.0 < wanted.1;
        let fine = size.0 as f32 > 2.0 * wanted.0.ceil() || size.1 as f32 > 2.0 * wanted.1.ceil();
        (coarse && !decoded.notes.has(Notes::SMALLER)) || (fine && !own)
    }
}

/// How far what is seen may lie outside what is decoded: the rounding of
/// a float, and no pixel of the picture.
const EDGE: f32 = 0.001;

/// What the picture is to show: the rectangle of it that lies in the
/// pane, in pixels of the picture, and how many pixels of a texture it
/// takes for one of the picture at the most: one, where it is shown at
/// its own size or larger.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Need {
    pub visible: [f32; 4],
    pub res: f32,
}

/// The largest scale a picture is zoomed to: pixels of the screen for
/// one of the picture.
pub const ZOOM_MAX: f32 = 8.0;
/// By how much one step of zooming changes the scale.
pub const ZOOM_STEP: f32 = 1.25;

/// How a picture lies in its room. Fitted, which it is when opened, it
/// is shown whole and never larger than it is. Zoomed it has a scale
/// and a point of the picture that lies in the middle of the room.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Look {
    /// Pixels of the screen for one of the picture, where it is not
    /// fitted.
    zoom: Option<f32>,
    /// Where in the picture the middle of the room lies.
    center: (f32, f32),
}

/// A side of a room as it is reckoned with: a pixel at the least, also
/// where it was said as nothing, as less, or as no number.
fn roomy(side: f32) -> f32 {
    if side >= 1.0 { side } else { 1.0 }
}

/// The scale at which the whole picture is in the room, and not above
/// one pixel for one.
pub fn fitted(picture: (u32, u32), room: (f32, f32)) -> f32 {
    let room = (roomy(room.0), roomy(room.1));
    let (w, h) = (picture.0.max(1) as f32, picture.1.max(1) as f32);
    (room.0 / w).min(room.1 / h).min(1.0)
}

/// How a picture lies, reckoned for a room: with which scale, and which
/// point of it is in the middle of the room.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub scale: f32,
    pub center: (f32, f32),
    pub room: (f32, f32),
    pub picture: (u32, u32),
}

impl Frame {
    /// Where a point of the picture lies, from the top left of the room.
    pub fn screen(&self, (x, y): (f32, f32)) -> (f32, f32) {
        (
            self.room.0 / 2.0 + (x - self.center.0) * self.scale,
            self.room.1 / 2.0 + (y - self.center.1) * self.scale,
        )
    }

    /// The rectangle of the picture that lies in the room, in pixels of
    /// the picture.
    pub fn visible(&self) -> [f32; 4] {
        let side = |center: f32, room: f32, length: u32| {
            let half = room / (2.0 * self.scale);
            let from = (center - half).max(0.0);
            (from, (center + half).min(length as f32) - from)
        };
        let (x, w) = side(self.center.0, self.room.0, self.picture.0);
        let (y, h) = side(self.center.1, self.room.1, self.picture.1);
        [x, y, w.max(0.0), h.max(0.0)]
    }

    /// What it takes to show it.
    pub fn need(&self) -> Need {
        Need { visible: self.visible(), res: self.scale.min(1.0) }
    }
}

impl Look {
    /// The zoom that is kept, which a room may have outgrown.
    #[cfg(test)]
    pub fn zoomed(&self) -> Option<f32> {
        self.zoom
    }

    /// The scale the picture is shown at in a room, where that is more
    /// than fitted: a zoom that the room has outgrown is none.
    pub fn shown(&self, picture: (u32, u32), room: (f32, f32)) -> Option<f32> {
        let scale = self.frame(picture, room).scale;
        self.zoom.filter(|_| scale > fitted(picture, room) * 1.000_1).map(|_| scale)
    }

    /// Gives up a zoom that the room has outgrown, so that what is said
    /// and what the next double click does are of what is shown.
    pub fn settle(&mut self, picture: (u32, u32), room: (f32, f32)) {
        if self.shown(picture, room).is_none() {
            *self = Self::default();
        }
    }

    /// How the picture lies in a room: a center that is held to what
    /// keeps the picture in the room, and in the middle where it is
    /// smaller than the room.
    pub fn frame(&self, picture: (u32, u32), room: (f32, f32)) -> Frame {
        let room = (roomy(room.0), roomy(room.1));
        let scale = self
            .zoom
            .map_or_else(|| fitted(picture, room), |zoom| zoom.clamp(fitted(picture, room), ZOOM_MAX));
        let held = |center: f32, room: f32, length: u32| {
            let length = length.max(1) as f32;
            let half = room / (2.0 * scale);
            if length <= 2.0 * half { length / 2.0 } else { center.clamp(half, length - half) }
        };
        let center = (held(self.center.0, room.0, picture.0), held(self.center.1, room.1, picture.1));
        Frame { scale, center, room, picture }
    }

    /// Zooms by `factor` about a point that is `anchor` from the middle
    /// of the room, so that the point of the picture under it stays
    /// there. At or below fitted it is fitted again.
    pub fn zoom_by(&mut self, factor: f32, picture: (u32, u32), room: (f32, f32), anchor: (f32, f32)) {
        // What is no number changes nothing: it would be kept as the
        // scale and end the next reckoning.
        if !(factor.is_finite() && factor > 0.0 && anchor.0.is_finite() && anchor.1.is_finite()) {
            return;
        }
        let before = self.frame(picture, room);
        let fit = fitted(picture, room);
        let scale = (before.scale * factor).clamp(fit, ZOOM_MAX);
        if scale <= fit * 1.000_1 {
            *self = Self::default();
            return;
        }
        let under = (before.center.0 + anchor.0 / before.scale, before.center.1 + anchor.1 / before.scale);
        self.zoom = Some(scale);
        self.center = (under.0 - anchor.0 / scale, under.1 - anchor.1 / scale);
        self.center = self.frame(picture, room).center;
    }

    /// Moves the picture by pixels of the screen, so that what is under
    /// the pointer goes with it. A picture that is fitted does not move.
    pub fn pan(&mut self, by: (f32, f32), picture: (u32, u32), room: (f32, f32)) {
        if self.zoom.is_none() || !by.0.is_finite() || !by.1.is_finite() {
            return;
        }
        let before = self.frame(picture, room);
        self.center = (before.center.0 - by.0 / before.scale, before.center.1 - by.1 / before.scale);
        self.center = self.frame(picture, room).center;
    }

    /// Fitted gives way to its own size, or to twice that where it is
    /// that already, about `anchor`; anything else goes back to fitted.
    pub fn toggle(&mut self, picture: (u32, u32), room: (f32, f32), anchor: (f32, f32)) {
        if self.zoom.is_some() {
            *self = Self::default();
            return;
        }
        let fit = fitted(picture, room);
        let target = if fit < 1.0 { 1.0 } else { 2.0 };
        self.zoom_by(target / fit, picture, room, anchor);
    }
}

/// When a picture is decoded again because its room became one it does
/// not fit, or the part of it that lies there is not decoded: a moment
/// after what is asked for last changed, not at every step of a window
/// or a hand that is dragged, and once for what is asked for. `K` is
/// what it is asked for: the room and the part of the picture.
#[derive(Debug)]
pub struct Again<K> {
    /// What is waited for, or what was asked for last.
    room: K,
    /// When it is decoded for that room, unless the room changes before.
    due: Option<Instant>,
}

impl<K: Copy + Default + PartialEq> Default for Again<K> {
    fn default() -> Self {
        Self { room: K::default(), due: None }
    }
}

impl<K: Copy + Default + PartialEq> Again<K> {
    /// Whether the picture is to be decoded for `room` now. `misfits`
    /// says whether what is shown does not fit that room.
    pub fn now(&mut self, misfits: bool, room: K, now: Instant, delay: Duration) -> bool {
        if !misfits {
            // What waited is given up, and the room it waited for with
            // it: when that room comes again, it is waited for anew. A
            // room that was asked for stays, see `asked`.
            if self.due.take().is_some() {
                self.room = K::default();
            }
            return false;
        }
        if self.room != room {
            (self.room, self.due) = (room, Some(now + delay));
        }
        self.due.is_some_and(|due| now >= due)
    }

    /// A decoding was asked for this room. It is not asked for the same
    /// room again: what comes back for it is what there is.
    pub fn asked(&mut self, room: K) {
        (self.room, self.due) = (room, None);
    }

    /// Nothing is waited for: a decoding is under way, or no picture is
    /// shown.
    pub fn rest(&mut self) {
        self.due = None;
    }

    /// When to wake for it.
    pub fn due(&self) -> Option<Instant> {
        self.due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_room_of_nothing_has_a_frame_as_well() {
        // Whoever draws holds the room to a pixel at the least. A frame
        // must not hang on that: a number that is none between two that
        // are none ends the app.
        let picture = (6238, 4162);
        for room in [(0.0, 0.0), (0.0, 700.0), (f32::NAN, 700.0), (-1.0, 700.0), (f32::INFINITY, 1.0)] {
            let frame = Look::default().frame(picture, room);
            assert!(frame.scale.is_finite() && frame.scale > 0.0, "{room:?}");
            assert!(frame.center.0.is_finite() && frame.center.1.is_finite(), "{room:?}");
        }
    }

    #[test]
    fn a_reason_is_read_back_from_its_byte_and_no_other() {
        for byte in 0..=u8::MAX {
            let reason = Refused::from_byte(byte);
            assert_eq!(reason.map(|reason| reason as u8), (1..=12).contains(&byte).then_some(byte));
        }
    }

    fn of(part: [u32; 4], size: (u32, u32), notes: u16) -> Seen {
        Seen {
            shown: Some((Decoded { picture: (6000, 4000), part, size, notes: Notes(notes) }, 1)),
            ..Seen::new(Format::Png)
        }
    }

    fn need(visible: [f32; 4], res: f32) -> Need {
        Need { visible, res }
    }

    #[test]
    fn a_picture_misfits_what_it_does_not_cover_or_is_too_coarse_or_far_too_fine_for() {
        let all = [0, 0, 6000, 4000];
        let reduced = of(all, (900, 600), 0);
        // What it was made for, a smaller one down to half, and not
        // below.
        let fitted = |res: f32| need([0.0, 0.0, 6000.0, 4000.0], res);
        assert!(!reduced.misfits(&fitted(0.15)) && !reduced.misfits(&fitted(0.075)));
        assert!(reduced.misfits(&fitted(0.07)), "far finer than needed");
        assert!(reduced.misfits(&fitted(0.2)), "too coarse");
        // At its own size it is never too fine, and one that was made
        // smaller for a limit is not made again for more.
        let own = of([1000, 500, 800, 600], (800, 600), 0);
        assert!(!own.misfits(&need([1100.0, 600.0, 200.0, 150.0], 0.1)));
        assert!(!own.misfits(&need([1100.0, 600.0, 200.0, 150.0], 1.0)));
        assert!(!of(all, (900, 600), Notes::SMALLER).misfits(&fitted(0.5)));
        assert!(!Seen::new(Format::Png).misfits(&fitted(1.0)), "nothing is shown");
        // A part is no good for what lies outside it, by more than the
        // rounding of where it was cut.
        assert!(!own.misfits(&need([1000.0, 500.0, 800.0, 600.0], 1.0)));
        assert!(!own.misfits(&need([999.9995, 500.0, 800.0, 600.0], 1.0)));
        // Half a pixel of the picture was let go, which is a strip of four
        // at 800 %.
        assert!(own.misfits(&need([999.6, 500.0, 800.4, 600.0], 1.0)));
        assert!(own.misfits(&need([999.0, 500.0, 800.0, 600.0], 1.0)));
        assert!(own.misfits(&need([1000.0, 500.0, 800.0, 601.0], 1.0)));
    }

    #[test]
    fn a_picture_is_fitted_never_larger_than_it_is_and_zooming_keeps_the_point_under_the_pointer() {
        let (picture, room) = ((6000, 4000), (1000.0, 700.0));
        assert!((fitted(picture, room) - 1000.0 / 6000.0).abs() < 1e-6);
        assert_eq!(fitted((300, 200), room), 1.0, "not larger than it is");
        let mut look = Look::default();
        let frame = look.frame(picture, room);
        assert_eq!(frame.center, (3000.0, 2000.0));
        assert_eq!(frame.visible(), [0.0, 0.0, 6000.0, 4000.0]);
        // Fitted does not move.
        look.pan((50.0, 50.0), picture, room);
        assert_eq!(look, Look::default());
        // In by the steps, about a point 100 right of the middle: what
        // is there stays there.
        let under = |look: &Look| {
            let frame = look.frame(picture, room);
            (frame.center.0 + 100.0 / frame.scale, frame.center.1 / 1.0)
        };
        let before = under(&look).0;
        for _ in 0..5 {
            look.zoom_by(ZOOM_STEP, picture, room, (100.0, 0.0));
        }
        let frame = look.frame(picture, room);
        assert!(frame.scale > fitted(picture, room));
        assert!((under(&look).0 - before).abs() < 1.0, "{} {before}", under(&look).0);
        // Not beyond the largest, and out again to fitted, which is
        // none that is kept.
        look.zoom_by(1e6, picture, room, (0.0, 0.0));
        assert_eq!(look.frame(picture, room).scale, ZOOM_MAX);
        look.zoom_by(1e-6, picture, room, (0.0, 0.0));
        assert_eq!(look, Look::default());
    }

    #[test]
    fn a_zoom_or_a_move_that_is_not_a_number_changes_nothing() {
        // NaN was kept as the scale and the next reckoning panicked, which
        // in a release build ends every session; as a move it stuck in the
        // center for good.
        let (picture, room) = ((6238, 4162), (1000.0, 700.0));
        let mut look = Look::default();
        look.zoom_by(2.0, picture, room, (0.0, 0.0));
        let held = look;
        for bad in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
            look.zoom_by(bad, picture, room, (0.0, 0.0));
            assert_eq!(look, held, "{bad}");
        }
        look.zoom_by(1.5, picture, room, (f32::NAN, 0.0));
        assert_eq!(look.zoomed(), held.zoomed(), "an anchor that is none");
        for by in [(f32::NAN, 0.0), (0.0, f32::INFINITY)] {
            look.pan(by, picture, room);
            assert_eq!(look, held);
        }
        assert!(look.frame(picture, room).center.0.is_finite());
    }

    #[test]
    fn what_is_said_of_the_zoom_is_the_scale_it_is_shown_at() {
        // A room that grew held the picture at fitted, and the line under
        // it said the old zoom.
        let picture = (6238, 4162);
        let (small, large) = ((1000.0, 700.0), (3000.0, 2100.0));
        let mut look = Look::default();
        for _ in 0..4 {
            look.zoom_by(ZOOM_STEP, picture, small, (0.0, 0.0));
        }
        let said = look.shown(picture, small).expect("zoomed");
        assert!((said - 0.3913).abs() < 0.01);
        assert_eq!(look.shown(picture, large), None, "it lies fitted in the larger room");
        // And the next double click is a click that does something.
        look.settle(picture, large);
        assert_eq!(look, Look::default());
    }

    #[test]
    fn a_picture_that_is_moved_stays_in_its_room_and_is_in_the_middle_where_it_is_smaller() {
        let (picture, room) = ((6000, 4000), (1000.0, 700.0));
        let mut look = Look::default();
        look.zoom_by(8.0, picture, room, (0.0, 0.0));
        look.toggle(picture, room, (0.0, 0.0));
        assert_eq!(look, Look::default(), "zoomed gives way to fitted");
        // At its own size, moved to the left and up for good.
        look.toggle(picture, room, (0.0, 0.0));
        assert_eq!(look.zoomed(), Some(1.0));
        look.pan((1e7, 1e7), picture, room);
        assert_eq!(look.frame(picture, room).visible(), [0.0, 0.0, 1000.0, 700.0]);
        look.pan((-1e7, -1e7), picture, room);
        assert_eq!(look.frame(picture, room).visible(), [5000.0, 3300.0, 1000.0, 700.0]);
        // One pixel of the picture is one of the room, and the middle of
        // the room lies where it is held to.
        assert_eq!(look.frame(picture, room).screen((5500.0, 3650.0)), (500.0, 350.0));
        // Zoomed at a picture that is smaller than the room, it lies in
        // the middle until it is larger than the room.
        let small = (300, 200);
        let mut look = Look::default();
        look.toggle(small, room, (0.0, 0.0));
        assert_eq!(look.zoomed(), Some(2.0), "its own size is where it is fitted: twice");
        assert_eq!(look.frame(small, room).visible(), [0.0, 0.0, 300.0, 200.0]);
        assert_eq!(look.frame(small, room).screen((0.0, 0.0)), (200.0, 150.0));
    }

    #[test]
    fn a_picture_is_decoded_again_a_moment_after_its_room_last_changed_and_once_for_a_room() {
        let (start, delay) = (Instant::now(), Duration::from_millis(200));
        let at = |millis: u64| start + Duration::from_millis(millis);
        let (small, large) = ((500, 300), (900, 600));
        let mut again = Again::default();
        // It fits: nothing is waited for.
        assert!(!again.now(false, small, at(0), delay) && again.due().is_none());
        // The room is another, and it does not fit: a moment later.
        assert!(!again.now(true, large, at(10), delay));
        assert_eq!(again.due(), Some(at(210)));
        assert!(!again.now(true, large, at(100), delay), "not before");
        // The room changes again before: the moment is put off.
        assert!(!again.now(true, (950, 600), at(150), delay));
        assert_eq!(again.due(), Some(at(350)));
        assert!(again.now(true, (950, 600), at(350), delay));
        // Asked for that room: what comes back for it is what there is,
        // also where it still does not fit.
        again.asked((950, 600));
        assert!(!again.now(true, (950, 600), at(400), delay) && again.due().is_none());
        assert!(!again.now(true, (950, 600), at(4000), delay));

        // The room grows, goes back before the moment came, and grows to
        // the very same again. What waited was given up, and the room it
        // waited for was held for one that had been asked for: nothing
        // waited again.
        let mut again = Again::default();
        assert!(!again.now(true, large, at(0), delay));
        assert!(!again.now(false, small, at(50), delay) && again.due().is_none());
        assert!(!again.now(true, large, at(100), delay));
        assert_eq!(again.due(), Some(at(300)));
        assert!(again.now(true, large, at(300), delay));
        // While a decoding is under way nothing is waited for.
        again.rest();
        assert!(again.due().is_none());
    }

    #[test]
    fn notes_that_are_not_known_are_none() {
        assert_eq!(Notes::known(Notes::ALPHA | Notes::SMALLER), Some(Notes(0b10_0001)));
        assert_eq!(Notes::known(1 << 6), None);
        assert!(Notes(Notes::EARLY).partial() && !Notes(Notes::ALPHA).partial());
    }
}
