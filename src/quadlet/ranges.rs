use rangemap::RangeSet;
use std::ops::Range;

#[derive(Clone, Debug, Eq, PartialEq)]
// TODO: investigate, if we actually need this struct
pub struct IdMap {
    inner: Range<u32>,
}

impl IdMap {
    pub fn new(start: u32, length: u32) -> Self {
        IdMap {
            inner: Range { start: start, end: start+length }
        }
    }

    pub fn length(&self) -> u32 {
        self.inner.end-self.inner.start
    }

    pub fn start(&self) -> u32 {
        self.inner.start
    }
}

impl From<Range<u32>> for IdMap {
    fn from(r: Range<u32>) -> Self {
        IdMap {
            inner: r
        }
    }
}

impl From<&Range<u32>> for IdMap {
    fn from(r: &Range<u32>) -> Self {
        // without clone() it'll recurse infinitely
        r.clone().into()
    }
}

#[derive(Clone)]
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

    pub fn is_empty(&self) -> bool {
        self.inner.iter().next().is_none()
    }

    pub fn iter(&self) -> impl Iterator<Item=IdMap> + '_ {
        self.inner.iter().map(IdMap::from)
    }

    pub fn new(start: u32, length: u32) -> Self {
        let mut ranges = Self::empty();
        ranges.add(start, length);
        ranges
    }

    pub fn parse(input: &str) -> Self {
        let mut ranges = Self::empty();
        for s in input.split(",") {
            let mut splits = s.splitn(2, "-");

            let start;
            let end;

            if let Some(start_s) = splits.next() {
                start = start_s.parse().unwrap_or(0);
                // FIXME: need to clamp to u32::MAX?

                end = if let Some(end_s) = splits.next() {
                    end_s.parse().unwrap_or(0)
                    // FIXME: need to clamp to u32::MAX?
                } else {
                    u32::MAX
                };
            } else {
                start = 0;
                end = u32::MAX;
            }

            if end >= start {
                ranges.add(start, u32::MAX.min((end - start).saturating_add(1)))
            }
        }

        ranges
    }

    pub fn remove(&mut self, start: u32, length: u32) {
        if length == 0 {
            return
        }

        self.inner.remove(start..start+length);
    }
}

// impl FromStr for IdRanges {
//     type Err;

//     fn from_str(s: &str) -> Result<Self, Self::Err> {
//         todo!()
//     }
// }

// impl IntoIterator for &IdRanges {
//     type Item = IdMap;

//     type IntoIter = Iterator<Item=Self::Item>;

//     fn into_iter(&self) -> Self::IntoIter {
//         self.iter()
//     }
// }

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

        mod is_empty {
            use super::*;

            #[test]
            fn true_for_empty() {
                let ranges = IdRanges::empty();

                assert!(ranges.is_empty());
            }

            #[test]
            fn false_for_non_empty() {
                let ranges = IdRanges::new(0, 1);

                assert!(!ranges.is_empty());
            }
        }

        mod parse {
            use super::*;


            #[test]
            fn with_single_number() {
                let input = "123";

                let ranges = IdRanges::parse(input);

                let mut iter = ranges.iter();
                assert_eq!(iter.next(), Some(IdMap::new(123, u32::MAX-123)));
                assert_eq!(iter.next(), None)
            }

            #[test]
            fn with_single_numeric_range() {
                let input = "123-456";

                let ranges = IdRanges::parse(input);

                let mut iter = ranges.iter();
                assert_eq!(iter.next(), Some(IdMap::new(123, 334)));
                assert_eq!(iter.next(), None)
            }

            #[test]
            fn with_numeric_range_and_number() {
                let input = "123-456,789";

                let ranges = IdRanges::parse(input);

                let mut iter = ranges.iter();
                assert_eq!(iter.next(), Some(IdMap::new(123, 334)));
                assert_eq!(iter.next(), Some(IdMap::new(789, u32::MAX-789)));
                assert_eq!(iter.next(), None)
            }

            #[test]
            fn with_multiple_numeric_ranges() {
                let input = "123-456,789-101112";

                let ranges = IdRanges::parse(input);

                let mut iter = ranges.iter();
                assert_eq!(iter.next(), Some(IdMap::new(123, 334)));
                assert_eq!(iter.next(), Some(IdMap::new(789, 100324)));
                assert_eq!(iter.next(), None)
            }

            #[test]
            fn merges_overlapping_non_monotonic_numeric_ranges() {
                let input = "123-456,345,234-567";

                let ranges = IdRanges::parse(input);

                let mut iter = ranges.iter();
                assert_eq!(iter.next(), Some(IdMap::new(123, u32::MAX-123)));
                assert_eq!(iter.next(), None)
            }

            #[test]
            fn with_borked_values() {
                let input = "123.456,-789";

                let ranges = IdRanges::parse(input);

                let mut iter = ranges.iter();
                assert_eq!(iter.next(), Some(IdMap::new(0, u32::MAX)));
                assert_eq!(iter.next(), None)
            }
        }

        mod remove {
            use super::*;
        }
    }
}