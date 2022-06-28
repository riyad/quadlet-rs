use rangemap::RangeSet;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IdMap {
    pub start: u32,
    pub length: u32,
}

impl IdMap {
    pub fn new(start: u32, length: u32) -> Self {
        IdMap {
            start: start,
            length: length,
        }
    }
}

impl From<&Range<u32>> for IdMap {
    fn from(r: &Range<u32>) -> Self {
        IdMap::new(r.start, r.end-r.start)
    }
}

pub struct IdRanges {
    inner: RangeSet<u32>,
}

impl IdRanges {
    pub fn add(&mut self, start: u32, length: u32) {
        if length == 0 {
            return
        }

        // The maximum value we can store is MAX-1, because if start
        // is 0 and length is MAX, then the first non-range item is
        // 0+MAX. So, we limit the start and length here so all
        // elements in the ranges are in this area.
        if start == u32::MAX {
            return
        }
        let length = length.min(u32::MAX - start);

        self.inner.insert(start..start+length)
    }

    pub fn empty() -> Self {
        IdRanges {
            inner: RangeSet::new(),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item=IdMap> + '_ {
        self.inner.iter().map(IdMap::from)
    }

    pub fn new(start: u32, length: u32) -> Self {
        let mut ranges = Self::empty();
        ranges.add(start, length);
        ranges
    }

    // pub fn parse(str: &str) -> Result<Self, Error> {
    //     todo!()
    // }

    pub fn remove(&mut self, start: u32, length: u32) {
        if length == 0 {
            return
        }

        self.inner.remove(start..start+length);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod id_ranges {
        use super::*;

        mod add {
            use super::*;

            #[test]
            fn with_length_of_zero_does_nothing() {
                let mut ranges = IdRanges::empty();

                ranges.add(1, 0);

                let mut iter = ranges.iter();
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn with_length_of_one() {
                let mut ranges = IdRanges::empty();

                ranges.add(1, 1);

                let mut iter = ranges.iter();
                assert_eq!(iter.next(), Some(IdMap::new(1, 1)));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn with_length_of_max_clamps() {
                let mut ranges = IdRanges::empty();

                ranges.add(10, u32::MAX);

                let mut iter = ranges.iter();
                assert_eq!(iter.next(), Some(IdMap::new(10, u32::MAX-10)));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn with_start_of_max_does_nothing() {
                let mut ranges = IdRanges::empty();

                ranges.add(u32::MAX, 1);

                let mut iter = ranges.iter();
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn with_start_of_zero() {
                let mut ranges = IdRanges::empty();

                ranges.add(0, 123);

                let mut iter = ranges.iter();
                assert_eq!(iter.next(), Some(IdMap::new(0, 123)));
                assert_eq!(iter.next(), None);
            }
        }
    }
}