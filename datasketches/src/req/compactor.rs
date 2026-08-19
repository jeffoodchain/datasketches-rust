// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

//! The relative compactor, which is the building block of the REQ sketch.

use std::cmp::Ordering;

use crate::common::random;
use crate::req::INIT_NUM_SECTIONS;
use crate::req::MIN_K;
use crate::req::MULTIPLIER;

/// Compares two items, treating incomparable pairs as equal.
///
/// The sketch rejects NaN before an item ever reaches a compactor, so on the values that
/// actually get stored `partial_cmp` is a total order and the fallback is unreachable.
pub(super) fn compare<T: PartialOrd>(a: &T, b: &T) -> Ordering {
    a.partial_cmp(b).unwrap_or(Ordering::Equal)
}

/// Rounds to the nearest even integer.
///
/// Mirrors C++ `req_compactor::nearest_even`. Section sizes are kept even so that a compacted
/// range always halves exactly.
fn nearest_even(value: f32) -> u32 {
    ((value / 2.0).round() as u32) << 1
}

/// Merge two ascending squences into one.
///
fn merge_sorted<T: PartialOrd>(a: Vec<T>, b: Vec<T>) -> Vec<T> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let mut ai = a.into_iter().peekable();
    let mut bi = b.into_iter().peekable();
    loop {
        match (ai.peek(), bi.peek()) {
            (Some(x), Some(y)) => {
                if compare(x, y) != Ordering::Greater {
                    out.push(ai.next().unwrap());
                } else {
                    out.push(bi.next().unwrap());
                }
            }
            (Some(_), None) => {
                out.extend(ai);
                break;
            }
            (None, Some(_)) => {
                out.extend(bi);
                break;
            }
            (None, None) => break,
        }
    }
    out
}

/// A single level of the REQ sketch.
///
/// Each compactor holds items of one fixed weight `2^lg_weight`. When it fills up, part of the
/// buffer is compacted: the range is halved by keeping every other item, and those survivors are
/// promoted to the next level where they count for twice as much.
///
/// Which part gets compacted is what makes the error relative rather than uniform. The buffer is
/// divided into `num_sections` sections, and a compaction only ever consumes the sections
/// furthest from the prioritized end of the rank domain.
#[derive(Debug, Clone)]
pub(super) struct ReqCompactor<T> {
    /// Items held by this compactor, in ascending order whenever `sorted` is true.
    items: Vec<T>,
    /// Base-2 logarithm of the weight each item in this compactor carries.
    lg_weight: u8,
    /// High rank accuracy: prioritizes high ranks when true, low ranks when false.
    hra: bool,
    /// Deterministic half of the compaction coin flip.
    coin: bool,
    /// Whether `items` is known to be in ascending order.
    sorted: bool,
    /// Section size before rounding, kept so repeated shrinking does not accumulate error.
    section_size_raw: f32,
    /// Current section size, always even.
    section_size: u32,
    /// Number of sections the buffer is divided into.
    num_sections: u8,
    /// Number of compactions performed, which drives the compaction schedule.
    state: u64,
}

impl<T: Clone + PartialOrd> ReqCompactor<T> {
    /// Creates an empty compactor.
    pub(super) fn new(hra: bool, lg_weight: u8, section_size: u32) -> Self {
        Self {
            items: Vec::new(),
            lg_weight,
            hra,
            coin: false,
            sorted: true,
            section_size_raw: section_size as f32,
            section_size,
            num_sections: INIT_NUM_SECTIONS,
            state: 0,
        }
    }

    /// Returns the number of items this compactor holds before it needs compacting.
    pub(super) fn nom_capacity(&self) -> u32 {
        MULTIPLIER * self.num_sections as u32 * self.section_size
    }

    /// Returns the retained items.
    pub(super) fn items(&self) -> &[T] {
        &self.items
    }

    /// Returns the number of items currently retained.
    pub(super) fn num_items(&self) -> u32 {
        self.items.len() as u32
    }

    /// Returns the base-2 logarithm of this compactor's item weight.
    pub(super) fn lg_weight(&self) -> u8 {
        self.lg_weight
    }

    /// Returns whether the items are known to be sorted.
    pub(super) fn is_sorted(&self) -> bool {
        self.sorted
    }

    /// Returns the compaction schedule state.
    pub(super) fn state(&self) -> u64 {
        self.state
    }

    /// Returns the current section size.
    pub(super) fn section_size(&self) -> u32 {
        self.section_size
    }

    /// Returns the current number of sections.
    pub(super) fn num_sections(&self) -> u8 {
        self.num_sections
    }

    /// Sorts the retained items if they are not already in order.
    pub(super) fn sort(&mut self) {
        if !self.sorted {
            self.items.sort_by(compare);
            self.sorted = true;
        }
    }

    /// Adds an item to this compactor.
    pub(super) fn append(&mut self, item: T) {
        self.items.push(item);
        // A buffer holding a single item is trivially sorted, so only clear the flag once there
        // are at least two items that could be out of order.
        if self.items.len() > 1 {
            self.sorted = false;
        }
    }

    /// Computes the weight of an item.
    pub(super) fn compute_weight(&mut self, item: &T, inclusive: bool) -> u64 {
        self.sort();
        let count = if inclusive {
            // <=
            self.items
                .partition_point(|x| compare(x, item) != Ordering::Greater)
        } else {
            // <
            self.items
                .partition_point(|x| compare(x, item) == Ordering::Less)
        };
        (count as u64) << self.lg_weight
    }

    pub(super) fn ensure_enough_sections(&mut self) -> bool {
        let ssr: f32 = self.section_size_raw / std::f32::consts::SQRT_2;
        let ne: u32 = nearest_even(ssr);
        if self.state >= (1u64 << (self.num_sections - 1)) && ne >= MIN_K.into() {
            self.section_size_raw = ssr;
            self.section_size = ne;
            self.num_sections <<= 1;
            return true;
        }
        false
    }

    //
    pub(super) fn compute_compaction_range(&self, secs_to_compact: u32) -> (u32, u32) {
        let mut non_compact = self.nom_capacity() / 2
            + (self.num_sections as u32 - secs_to_compact) * self.section_size;
        if (self.num_items() - non_compact) % 2 == 1 {
            non_compact += 1;
        }

        if self.hra {
            (0, self.num_items() - non_compact)
        } else {
            (non_compact, self.num_items())
        }
    }

    pub(super) fn compact(&mut self, next: &mut Self) -> (u32, u32) {
        let starting_nom_capacity: u32 = self.nom_capacity();
        let secs_to_compact: u32 =
            std::cmp::min((!self.state).trailing_zeros() + 1, self.num_sections as u32);
        let (low, high) = self.compute_compaction_range(secs_to_compact);
        if high - low < 2 {
            panic!("Reqsketches: compaction range error");
        }

        if self.state % 2 == 1 {
            self.coin = !self.coin;
        }
        // odd flip coin
        else {
            self.coin = random::random_bit();
        } // random coin flip

        let removed: Vec<T> = self.items.drain(low as usize..high as usize).collect();
        let num = (high - low) / 2;
        let promoted: Vec<T> = removed
            .into_iter()
            .skip(usize::from(self.coin))
            .step_by(2)
            .collect();

        next.items = merge_sorted(std::mem::take(&mut next.items), promoted);
        next.sorted = true;

        self.state += 1;
        self.ensure_enough_sections();

        (num, self.nom_capacity() - starting_nom_capacity)
    }

    pub(super) fn merge(&mut self, other: &Self) {
        debug_assert_eq!(
            self.lg_weight, other.lg_weight,
            "compactors of different weights cannot be merged"
        );
        self.state |= other.state;
        // A merged state can jump past several growth threshold at once, and each
        // call grows by at most one level, so keep going till the sections settle
        while self.ensure_enough_sections() {}
        self.sort();
        let mut incoming = other.items.clone();
        if !other.sorted {
            incoming.sort_by(compare);
        }
        
        self.items = merge_sorted(std::mem::take(&mut self.items), incoming);
        self.sorted = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fills a compactor to its nominal capacity with ascending values.
    fn filled(hra: bool, lg_weight: u8, section_size: u32, n: u32) -> ReqCompactor<f32> {
        let mut c = ReqCompactor::new(hra, lg_weight, section_size);
        for i in 0..n {
            c.append(i as f32);
        }
        c.sort();
        c
    }

    /// Total weight held by a compactor (items count * 2^lg_weight)
    fn total_weight(c: &ReqCompactor<f32>) -> u64 {
        (c.num_items() as u64) << c.lg_weight()
    }

    #[test]
    fn test_nearest_even() {
        assert_eq!(nearest_even(0.0), 0);
        assert_eq!(nearest_even(1.0), 2);
        assert_eq!(nearest_even(3.0), 4);
        assert_eq!(nearest_even(4.0), 4);
        assert_eq!(nearest_even(5.9), 6);
    }

    #[test]
    fn test_merge_sorted_empty_sides() {
        assert_eq!(merge_sorted(vec![1.0, 2.0], Vec::new()), vec![1.0, 2.0]);
        assert_eq!(merge_sorted(Vec::new(), vec![1.0, 2.0]), vec![1.0, 2.0]);
        assert_eq!(
            merge_sorted(Vec::<f32>::new(), Vec::new()),
            Vec::<f32>::new()
        );
    }

    #[test]
    fn test_hra_compacts_the_low_end() {
        let mut c = filled(true, 0, 12, 72);
        assert_eq!(c.compute_compaction_range(1), (0, 12));
        assert_eq!(c.compute_compaction_range(2), (0, 24));
        assert_eq!(c.compute_compaction_range(3), (0, 36));
    }

    #[test]
    fn test_lra_mirrors_hra() {
        let mut c = filled(false, 0, 12, 72);
        assert_eq!(c.compute_compaction_range(1), (60, 72));
        assert_eq!(c.compute_compaction_range(2), (48, 72));
        assert_eq!(c.compute_compaction_range(3), (36, 72));
    }

    #[test]
    fn test_compact_preserves_total_weight() {
        let mut c = filled(true, 0, 12, 72);
        let mut next = ReqCompactor::new(true, 1, 12);
        let before = total_weight(&c) + total_weight(&next);
        c.compact(&mut next);
        assert_eq!(total_weight(&c) + total_weight(&next), before, "compaction must preserve total weight exactly");
    }
}
