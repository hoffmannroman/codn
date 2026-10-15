//! Pictures: a file made into pixels, in a process of its own. What a
//! file holds must not be able to end the app, and a decoder that fails,
//! loops or asks for memory without end then ends that process and no
//! session.
//!
//! `child` is that process, codn itself started with `ARGUMENT`, `client`
//! the thread of the app that has it, and `wire` what it is told and what
//! it answers. `png` and `jpeg` are the decoders. Nothing in here knows
//! the window or the renderer.

mod child;
pub mod client;
mod jpeg;
mod png;
pub mod wire;

pub use child::child;
pub use png::fit;

use crate::core::picture::Need;
use wire::Wish;

/// What `Again` waits by for a wish: the room, the part, and the room the
/// part is fitted into, which is its scale. Two wishes for the same part
/// at another scale are two.
pub fn key(room: (u32, u32), wish: &Wish) -> [u32; 8] {
    let [x, y, w, h] = wish.part;
    [room.0, room.1, wish.room.0, wish.room.1, x, y, w, h]
}

/// Whether a picture that is shown is what `wish` would give: the same
/// part at the same size. Where the room and the texture side leave no
/// other size, "too fine" has nothing to ask for.
pub fn is_answer(decoded: &crate::core::picture::Decoded, wish: &Wish) -> bool {
    let part = if wish.part == [0; 4] { [0, 0, decoded.picture.0, decoded.picture.1] } else { wish.part };
    decoded.part == part && decoded.size == fit((part[2], part[3]), wish)
}

/// What to ask for to show `need` of a picture of this size, with
/// `base` for what does not depend on it: the whole picture, as `fit`
/// has it, where that has all the pixels `need` takes, and else the
/// rectangle that is seen with as much around it again, which is
/// decoded at the scale that is needed and no larger than its own, so
/// that a hand moving the picture a little draws and asks for nothing.
pub fn ask(picture: (u32, u32), need: &Need, base: &Wish) -> Wish {
    let whole = Wish { part: [0; 4], ..*base };
    let size = fit(picture, &whole);
    let wanted = (picture.0 as f32 * need.res, picture.1 as f32 * need.res);
    if size.0 as f32 + 1.0 >= wanted.0 && size.1 as f32 + 1.0 >= wanted.1 {
        return whole;
    }
    let res = need.res.max(f32::MIN_POSITIVE);
    // From where, and how long, the rectangle on one axis: about the
    // middle of what is seen, twice as long, and no longer than a
    // texture takes.
    let along = |at: f32, seen: f32, length: u32| {
        let cap = (base.side as f32 / res).floor().max(seen.ceil());
        let wide = (2.0 * seen).min(cap).min(length as f32);
        let from = (at + seen / 2.0 - wide / 2.0).clamp(0.0, (length as f32 - wide).max(0.0)).floor();
        let to = (from + wide.ceil()).max((at + seen).ceil()).min(length as f32);
        (from as u32, (to - from).max(1.0) as u32)
    };
    let [x, y, w, h] = need.visible;
    let ((px, pw), (py, ph)) = (along(x, w, picture.0), along(y, h, picture.1));
    let room = ((pw as f32 * res).ceil().max(1.0) as u32, (ph as f32 * res).ceil().max(1.0) as u32);
    Wish { room, whole: 0, part: [px, py, pw, ph], ..*base }
}

/// What codn is started with to be the process for one picture.
pub const ARGUMENT: &str = "--picture";

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use crate::core::picture::{Again, Decoded, Format, Look, Notes, Seen, ZOOM_STEP};
    use wire::WHOLE;

    fn base() -> Wish {
        Wish { format: Format::Png, room: (1000, 700), side: 8192, whole: WHOLE, part: [0; 4] }
    }

    /// What the process would answer to a wish for `picture`.
    fn answered(picture: (u32, u32), wish: &Wish) -> Decoded {
        let part = if wish.part == [0; 4] { [0, 0, picture.0, picture.1] } else { wish.part };
        Decoded { picture, part, size: fit((part[2], part[3]), wish), notes: Notes(0) }
    }

    #[test]
    fn what_is_shown_is_not_asked_for_again_where_the_wish_gives_the_same() {
        // A picture wider than a texture is decoded whole at the side,
        // which is "too fine" for a room, and a room one pixel wider asked
        // for it again.
        let picture = (9000, 100);
        let side = Wish { side: 8192, ..base() };
        let (small, large) = (Wish { room: (1000, 700), ..side }, Wish { room: (1001, 700), ..side });
        let shown = answered(picture, &small);
        assert_eq!(shown.size, (8192, 91));
        assert!(is_answer(&shown, &large) && is_answer(&shown, &small));
        // Another room that gives another size is another picture.
        let other = Wish { room: (100, 700), side: 8192, whole: 0, ..base() };
        assert!(!is_answer(&shown, &other));
        // A part is the same part, or it is none.
        let cut = answered(picture, &Wish { part: [10, 0, 500, 100], ..small });
        assert!(is_answer(&cut, &Wish { part: [10, 0, 500, 100], ..small }));
        assert!(!is_answer(&cut, &Wish { part: [11, 0, 500, 100], ..small }));
    }

    #[test]
    fn every_step_of_zooming_in_from_fitted_is_decoded_also_where_the_part_is_the_same() {
        // The first three steps in from fitted ask for the whole rectangle
        // as a part, each at a finer scale, and the key that `Again` waits
        // by held the part and not the scale: the second and the third were
        // never asked.
        let (picture, room) = ((6238, 4162), (1000u32, 700u32));
        let roomf = (room.0 as f32, room.1 as f32);
        let base = Wish { room, ..base() };
        let mut look = Look::default();
        let first = ask(picture, &look.frame(picture, roomf).need(), &base);
        let mut again = Again::<[u32; 8]>::default();
        again.asked(key(room, &first));
        let mut seen = Seen { shown: Some((answered(picture, &first), 1)), ..Seen::new(Format::Png) };
        let delay = Duration::from_millis(200);
        let mut now = Instant::now();
        let mut parts = std::collections::HashSet::new();
        for step in 1..=3 {
            look.zoom_by(ZOOM_STEP, picture, roomf, (0.0, 0.0));
            let need = look.frame(picture, roomf).need();
            let wish = ask(picture, &need, &base);
            parts.insert(wish.part);
            assert!(seen.misfits(&need), "step {step}: what is shown is too coarse");
            assert!(!again.now(true, key(room, &wish), now, delay));
            now += delay;
            assert!(again.now(true, key(room, &wish), now, delay), "step {step} is decoded");
            again.asked(key(room, &wish));
            seen.shown = Some((answered(picture, &wish), 2 + step));
            assert!(!seen.misfits(&need), "step {step}: what came back fits");
        }
        assert_eq!(parts.len(), 1, "the same part each time, or this tests nothing");
    }

    #[test]
    fn the_whole_picture_is_asked_for_where_it_has_the_pixels_that_are_needed() {
        // Small enough to be kept whole: any scale is shown from it.
        let small = Need { visible: [100.0, 100.0, 125.0, 87.0], res: 1.0 };
        assert_eq!(ask((2000, 1400), &small, &base()).part, [0; 4]);
        // Fitted into the room, a large one: the whole, as it was.
        let fitted = Need { visible: [0.0, 0.0, 6000.0, 4000.0], res: 1000.0 / 6000.0 };
        assert_eq!(ask((6000, 4000), &fitted, &base()).part, [0; 4]);
    }

    #[test]
    fn a_large_picture_asked_for_at_its_own_size_is_a_rectangle_around_what_is_seen() {
        let need = Need { visible: [2000.0, 1500.0, 1000.0, 700.0], res: 1.0 };
        let wish = ask((6000, 4000), &need, &base());
        // Twice as long each way, about the middle of what is seen, at
        // its own size and not kept whole.
        assert_eq!(wish.part, [1500, 1150, 2000, 1400]);
        assert_eq!((wish.room, wish.whole), ((2000, 1400), 0));
        // Where it reaches the edge, it is moved in and not cut short.
        let edge = Need { visible: [5000.0, 3300.0, 1000.0, 700.0], res: 1.0 };
        assert_eq!(ask((6000, 4000), &edge, &base()).part, [4000, 2600, 2000, 1400]);
        let corner = Need { visible: [0.0, 0.0, 1000.0, 700.0], res: 1.0 };
        assert_eq!(ask((6000, 4000), &corner, &base()).part, [0, 0, 2000, 1400]);
    }

    #[test]
    fn a_rectangle_is_asked_for_at_the_scale_that_is_needed_and_within_a_texture() {
        // Zoomed out a little from its own size, a larger rectangle at
        // a lower scale.
        let need = Need { visible: [500.0, 500.0, 2500.0, 1750.0], res: 0.4 };
        let wish = ask((6000, 4000), &need, &base());
        assert_eq!(wish.part, [0, 0, 5000, 3500]);
        assert_eq!(wish.room, (2000, 1400));
        // What would be longer than a texture at that scale is cut.
        let little = Wish { side: 1024, ..base() };
        let wish = ask((6000, 4000), &Need { visible: [100.0, 100.0, 500.0, 350.0], res: 1.0 }, &little);
        assert_eq!(wish.part[2..], [1000, 700]);
        assert!(wish.room.0 <= 1024);
        // Everything that is asked for is inside the picture and holds
        // what is seen.
        for x in [0.0, 1.0, 2999.5, 5000.0] {
            let need = Need { visible: [x, 10.0, 1000.0, 700.0], res: 1.0 };
            let [px, py, pw, ph] = ask((6000, 4000), &need, &base()).part;
            assert!(px as f32 <= x && (px + pw) as f32 >= x + 1000.0, "at {x}");
            assert!(py as f32 <= 10.0 && (py + ph) as f32 >= 710.0 && px + pw <= 6000 && py + ph <= 4000);
        }
    }
}
