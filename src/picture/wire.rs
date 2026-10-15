//! What the process for pictures is told and what it answers. It is told
//! numbers on its command line and given the file as its input. It
//! answers once, when it is done: a head of fixed length, and after a
//! head that says so the pixels, four bytes each, multiplied by their
//! alpha. Every number is held against a limit before memory is asked
//! for by it, on both sides.

use std::ffi::OsString;

use crate::core::picture::{Decoded, Format, Notes, Refused};

/// Most bytes of a file that are read.
pub const FILE_MAX: u64 = 64 << 20;
/// The longest side a picture may have, and the most pixels: what is
/// over either is turned away at its head, before any memory.
pub const SIDE_MAX: u32 = 65_535;
pub const PIXELS_MAX: u64 = 1 << 27;
/// Up to how many pixels a picture is kept whole: 32 MiB, which takes in
/// a picture of a 4K screen.
pub const WHOLE: u32 = 1 << 23;
/// The least room a picture is fitted into, a side. A pane is never that
/// small, and no field of a result then gathers more pixels than its
/// sums hold.
pub const ROOM_MIN: u32 = 16;
/// The length of a head.
pub const HEAD: usize = 40;

const MARK: [u8; 4] = *b"cdnp";
const VERSION: u8 = 2;

/// What is wanted of a picture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Wish {
    pub format: Format,
    /// The room it is to fit into, in pixels of the screen.
    pub room: (u32, u32),
    /// The longest side a texture may have.
    pub side: u32,
    /// Up to how many pixels the picture is wanted whole, whatever the
    /// room.
    pub whole: u32,
    /// The rectangle of the picture that is wanted: where it begins and
    /// its size. All zero is the whole of it. What of it lies outside
    /// the picture is cut off, and a rectangle with nothing inside the
    /// picture is the whole of it. The room is what that rectangle is
    /// fitted into, and `whole` what it is kept at its own size up to.
    pub part: [u32; 4],
}

impl Wish {
    /// The rectangle of a picture of this size that the wish is for: the
    /// one it names, cut to lie within the picture, or the whole of it
    /// where it names none or none that has anything inside the picture.
    /// Both decoders cut by it, and an answer is held against it.
    pub fn part_of(&self, picture: (u32, u32)) -> [u32; 4] {
        let [x, y, w, h] = self.part;
        let (right, below) = (x.saturating_add(w).min(picture.0), y.saturating_add(h).min(picture.1));
        if self.part == [0; 4] || x >= right || y >= below {
            return [0, 0, picture.0, picture.1];
        }
        [x, y, right - x, below - y]
    }

    /// What the process is started with, after the argument that makes
    /// codn that process.
    pub fn arguments(&self) -> Vec<String> {
        let numbers = [self.room.0, self.room.1, self.side, self.whole].into_iter().chain(self.part);
        std::iter::once(self.format.word().to_owned()).chain(numbers.map(|n| n.to_string())).collect()
    }

    /// The wish in those arguments, if they are one: a format and eight
    /// numbers in digits, and nothing more.
    pub fn read(arguments: impl Iterator<Item = OsString>) -> Option<Self> {
        let words: Vec<OsString> = arguments.take(10).collect();
        let [format, numbers @ ..] = words.as_slice() else { return None };
        let format = Format::from_word(format.to_str()?)?;
        let number = |word: &OsString| {
            let word = word.to_str()?;
            (!word.is_empty() && word.len() <= 10 && word.bytes().all(|b| b.is_ascii_digit()))
                .then(|| word.parse::<u32>().ok())?
        };
        let numbers: Vec<u32> = numbers.iter().map(number).collect::<Option<_>>()?;
        let [w, h, side, whole, part @ ..] = numbers.as_slice() else { return None };
        let part: [u32; 4] = part.try_into().ok()?;
        (*w > 0 && *h > 0 && *side > 0).then_some(Self {
            format,
            room: (*w, *h),
            side: *side,
            whole: *whole,
            part,
        })
    }

    /// Whether a picture of this size can be the answer to the wish: no
    /// side longer than a texture takes, and no more pixels than the room
    /// has or than are wanted whole. Asked before memory is asked for by
    /// what a head says.
    pub fn takes(&self, size: (u32, u32)) -> bool {
        let (w, h) = (u64::from(size.0), u64::from(size.1));
        let room = u64::from(self.room.0.max(ROOM_MIN)).saturating_mul(u64::from(self.room.1.max(ROOM_MIN)));
        w > 0
            && h > 0
            && size.0 <= self.side
            && size.1 <= self.side
            && w.saturating_mul(h) <= room.max(u64::from(self.whole))
    }
}

/// How it went, as the head of an answer says it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Head(pub Result<Decoded, Refused>);

impl Head {
    pub fn write(&self) -> [u8; HEAD] {
        let mut bytes = [0; HEAD];
        let (how, notes, picture, part, size) = match self.0 {
            Ok(Decoded { picture, part, size, notes }) => (0, notes.0, picture, part, size),
            Err(reason) => (reason as u8, 0, (0, 0), [0; 4], (0, 0)),
        };
        let [low, high] = notes.to_le_bytes();
        let fields =
            [picture.0, picture.1, part[0], part[1], part[2], part[3], size.0, size.1].map(u32::to_le_bytes);
        let parts = MARK.into_iter().chain([VERSION, how, low, high]).chain(fields.into_iter().flatten());
        bytes.iter_mut().zip(parts).for_each(|(byte, part)| *byte = part);
        bytes
    }

    /// The head in these bytes, if they are one. Nothing in it is held
    /// against a wish yet: see `Wish::takes`.
    pub fn read(bytes: &[u8; HEAD]) -> Option<Self> {
        let (mark, rest) = bytes.split_first_chunk::<4>()?;
        let ([version, how, low, high], rest) = rest.split_first_chunk::<4>()?;
        if *mark != MARK || *version != VERSION {
            return None;
        }
        let mut fields = rest.as_chunks::<4>().0.iter().map(|field| u32::from_le_bytes(*field));
        let mut next = || fields.next();
        let picture = (next()?, next()?);
        let part = [next()?, next()?, next()?, next()?];
        let size = (next()?, next()?);
        let notes = Notes::known(u16::from_le_bytes([*low, *high]))?;
        match *how {
            0 => {
                // A picture has a size, its part lies within it, and what
                // was made of the part is no larger than the part.
                let within = |at: u32, length: u32, of: u32| {
                    length > 0 && at.checked_add(length).is_some_and(|end| end <= of)
                };
                // No more of it than a file may hold: the decoders turn
                // such a file away, so a process that says one is none.
                let pixels = u64::from(picture.0) * u64::from(picture.1);
                let sound = picture.0 <= SIDE_MAX
                    && picture.1 <= SIDE_MAX
                    && pixels <= PIXELS_MAX
                    && within(part[0], part[2], picture.0)
                    && within(part[1], part[3], picture.1)
                    && size.0 > 0
                    && size.1 > 0
                    && size.0 <= part[2]
                    && size.1 <= part[3];
                sound.then_some(Self(Ok(Decoded { picture, part, size, notes })))
            }
            how => {
                let nothing = notes.0 == 0 && picture == (0, 0) && part == [0; 4] && size == (0, 0);
                Refused::from_byte(how).filter(|_| nothing).map(|reason| Self(Err(reason)))
            }
        }
    }
}

/// How many bytes of pixels follow a head for a picture of this size.
pub fn bytes_of(size: (u32, u32)) -> Option<usize> {
    usize::try_from(u64::from(size.0).checked_mul(u64::from(size.1))?.checked_mul(4)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wish() -> Wish {
        Wish { format: Format::Png, room: (1000, 667), side: 16384, whole: WHOLE, part: [0; 4] }
    }

    #[test]
    fn a_wish_is_read_back_from_its_arguments() {
        let told = wish();
        assert_eq!(told.arguments(), ["png", "1000", "667", "16384", "8388608", "0", "0", "0", "0"]);
        let read = |words: &[&str]| Wish::read(words.iter().map(OsString::from));
        let words = told.arguments();
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        assert_eq!(read(&words), Some(told));
        let part = Wish { part: [10, 20, 300, 400], ..told };
        assert_eq!(Wish::read(part.arguments().into_iter().map(OsString::from)), Some(part));

        // One too few, one too many, a format that is none, a number
        // that is none, a room without a side, no texture at all.
        assert_eq!(read(&words[..8]), None);
        assert_eq!(read(&[&words[..], &["0"]].concat()), None);
        for (at, word) in
            [(0, "gif"), (1, "+1"), (1, "-1"), (2, "1e3"), (2, ""), (3, "99999999999"), (4, " 1")]
        {
            let mut wrong = words.clone();
            wrong[at] = word;
            assert_eq!(read(&wrong), None, "{word:?} at {at}");
        }
        for at in 1..=3 {
            let mut wrong = words.clone();
            wrong[at] = "0";
            assert_eq!(read(&wrong), None, "nothing at {at}");
        }
    }

    #[test]
    fn a_head_is_read_back_and_nothing_else_is_one() {
        let decoded = Decoded::whole((6238, 4162), (1000, 667), Notes(Notes::ALPHA));
        let cut = Decoded { part: [100, 200, 3000, 2000], size: (1500, 1000), ..decoded };
        for head in [Head(Ok(decoded)), Head(Ok(cut)), Head(Err(Refused::Apple)), Head(Err(Refused::Memory))]
        {
            assert_eq!(Head::read(&head.write()), Some(head));
        }
        let sound = Head(Ok(decoded)).write();
        let changed = |at: usize, to: u8| {
            let mut bytes = sound;
            bytes[at] = to;
            Head::read(&bytes)
        };
        assert_eq!(changed(0, b'x'), None, "another mark");
        assert_eq!(changed(4, 3), None, "another version");
        assert_eq!(changed(7, 0x80), None, "a note that is not known");
        // A part that is empty, or reaches out of the picture, or one
        // that is larger than the picture itself made of it.
        let part = |part: [u32; 4]| Head::read(&Head(Ok(Decoded { part, ..decoded })).write());
        assert_eq!(part([0, 0, 0, 4162]), None);
        assert_eq!(part([1, 0, 6238, 4162]), None);
        assert_eq!(part([u32::MAX, 0, 2, 4162]), None);
        assert_eq!(part([0, 0, 999, 4162]), None, "smaller than what was made of it");
        assert_eq!(changed(5, 200), None, "a reason that is not known");
        assert_eq!(changed(5, Refused::Apple as u8), None, "a reason with a picture");
        // No size, and one larger than the picture it is of.
        assert_eq!(Head::read(&Head(Ok(Decoded { size: (0, 667), ..decoded })).write()), None);
        assert_eq!(Head::read(&Head(Ok(Decoded { size: (6239, 667), ..decoded })).write()), None);
    }

    #[test]
    fn a_head_that_says_a_picture_no_file_may_hold_is_none() {
        // The decoders turn such a file away. A process that says it all
        // the same is not a decoder any more, and the size it says is
        // shown under the picture and reckoned with.
        let one = |picture| Decoded { picture, part: [0, 0, 1, 1], size: (1, 1), notes: Notes(0) };
        let read = |picture| Head::read(&Head(Ok(one(picture))).write());
        assert_eq!(read((u32::MAX, u32::MAX)), None);
        assert_eq!(read((SIDE_MAX + 1, 1)), None);
        assert_eq!(read((1, SIDE_MAX + 1)), None);
        assert_eq!(read((SIDE_MAX, SIDE_MAX)), None, "more pixels than a picture may have");
        // The largest there may be.
        let high = (PIXELS_MAX / u64::from(SIDE_MAX)) as u32;
        assert!(read((SIDE_MAX, high)).is_some() && read((SIDE_MAX, high + 1)).is_none());
    }

    #[test]
    fn an_answer_is_held_against_what_was_asked() {
        let wish = wish();
        // What fits the room, and what is whole and no larger than that.
        assert!(wish.takes((1000, 667)) && wish.takes((3840, 2160)) && wish.takes((1, 1)));
        // More than is kept whole, a side no texture takes, no size.
        assert!(!wish.takes((4096, 2049)));
        assert!(!Wish { side: 2048, ..wish }.takes((2049, 10)));
        assert!(!wish.takes((0, 10)) && !wish.takes((10, 0)));
        // Nothing is wanted whole: then the room alone counts.
        let far = Wish { whole: 0, ..wish };
        assert!(far.takes((1000, 667)) && !far.takes((1001, 668)));
        assert_eq!(bytes_of((1000, 667)), Some(2_668_000));
        assert_eq!(bytes_of((u32::MAX, u32::MAX)), None);
    }
}
