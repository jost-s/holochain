//! The Quantized DHT Arc type
//!
//! Arq coordinates are expressed in terms of powers-of-two, representing
//! the "chunk" or "segment" size to work with. The actual extent of the Arq
//! is expressed as a `start` and an `offset`, in terms of the chunk size.
//! So, Arq boundaries can only ever fall on a quantized grid which is determined
//! by the `power` setting.

mod arq_set;
mod peer_view;
mod strat;

#[cfg(feature = "test_utils")]
pub mod ascii;

pub use arq_set::*;

pub use peer_view::*;
pub use strat::*;

use kitsune_p2p_dht_arc::{DhtArc, DhtArcRange};

use crate::{op::Loc, spacetime::*};

/// Convenience method for taking the power of 2 in u32
pub fn pow2(p: u8) -> u32 {
    debug_assert!(p < 32);
    2u32.pow((p as u32).min(31))
}

/// Convenience method for taking the power of 2 in f64
pub fn pow2f(p: u8) -> f64 {
    debug_assert!(p < 32);
    2f64.powf(p as f64)
}

/// Maximum number of values that a u32 can represent.
pub(crate) const U32_LEN: u64 = u32::MAX as u64 + 1;

/// Represents the start point or "left edge" of an Arq.
///
/// This helps us generalize over the two use cases of Arq:
/// 1. An Arq which is defined at a definite absolute DhtLocation corresponding
///    to an Agent's location, and which can be requantized, resized, etc.
/// 2. An Arq which has no absolute location defined, and which simply represents
///    a (quantized) range.
pub trait ArqStart: Sized + Copy + std::fmt::Debug {
    /// Get the DhtLocation representation
    fn to_loc(&self, dim: impl SpaceDim, power: u8) -> Loc;
    /// Get the exponential SpaceOffset representation
    fn to_offset(&self, dim: impl SpaceDim, power: u8) -> SpaceOffset;
    /// Requantize to a higher power, using the precalculated multiplicative factor.
    fn requantize_up(&self, factor: u32) -> Option<Self>;
    /// Requantize to a lower power, using the precalculated multiplicative factor.
    fn requantize_down(&self, factor: u32) -> Self;
    /// Zero value
    fn zero() -> Self;
}

impl ArqStart for Loc {
    fn to_loc(&self, _dim: impl SpaceDim, _power: u8) -> Loc {
        *self
    }

    fn to_offset(&self, dim: impl SpaceDim, power: u8) -> SpaceOffset {
        SpaceOffset::from_absolute_rounded(*self, dim.get(), power)
    }

    fn requantize_up(&self, _factor: u32) -> Option<Self> {
        Some(*self)
    }

    fn requantize_down(&self, _factor: u32) -> Self {
        *self
    }

    fn zero() -> Self {
        0.into()
    }
}

impl ArqStart for SpaceOffset {
    fn to_loc(&self, dim: impl SpaceDim, power: u8) -> Loc {
        self.to_absolute(dim.get(), power)
    }

    fn to_offset(&self, _dim: impl SpaceDim, _power: u8) -> SpaceOffset {
        *self
    }

    fn requantize_up(&self, factor: u32) -> Option<Self> {
        ((**self) % factor == 0).then(|| *self / factor)
    }

    fn requantize_down(&self, factor: u32) -> Self {
        *self * factor
    }

    fn zero() -> Self {
        0.into()
    }
}

/// A quantized DHT arc.
///
/// ## Coordinates
///
/// Arq coordinates are expressed in terms of powers-of-two, representing
/// the "chunk" or "segment" size to work with.
/// The chunk size is determined by the [`Topology`] of the space it is in,
/// as well as the `power` of the Arq. The actual chunk size is given by:
///
/// `chunk size = topology.space.quantum * 2^power`
///
/// So, the `power` represents the amount of quantization *on top* of the quantum
/// size set by the Topology, not the total quantization level.
///
/// The `start` is generic, because there are actually two flavors of Arq:
/// - one which has a definite starting DhtLocation associated with it,
/// - and one which does not.
///
/// The first flavor is used to represent Arqs which belong to Agents. It's important
/// to record the actual absolute Location of the Arq, because the exact location
/// determines the starting Chunk when requantizing to higher and lower levels.
///
/// The second flavor is mainly used to represent the intersections and unions of Arqs.
/// In this case, there is no definite location associated, so we want to forget
/// about the original Location data associated with each Arq.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "fuzzing",
    derive(arbitrary::Arbitrary, proptest_derive::Arbitrary)
)]
pub struct Arq<S: ArqStart = Loc> {
    /// The "start" defines the left edge of the arq
    pub start: S,
    /// The level of quantization. Total length is `2^power * count`.
    /// The power must be between 0 and 31, inclusive (power of 32 causes overflow)
    pub power: u8,
    /// The number of unit lengths.
    /// We never expect the count to be less than 4 or so, and not much larger
    /// than 32.
    pub count: SpaceOffset,
}

/// Alias for Arq with an Loc start
pub type ArqLocated = Arq<Loc>;

/// Alias for Arq with an SpaceOffset start
pub type ArqBounds = Arq<SpaceOffset>;

impl<S: ArqStart> Arq<S> {
    /// Constructor from individual parts
    pub fn new(power: u8, start: S, count: SpaceOffset) -> Self {
        Self {
            power,
            start,
            count,
        }
    }

    /// The number of quanta to use for each segment
    #[inline]
    fn quantum_chunk_width(&self) -> u32 {
        pow2(self.power)
    }

    /// The absolute length of each segment, the "chunk size"
    #[inline]
    pub(crate) fn absolute_chunk_width(&self, dim: impl SpaceDim) -> u32 {
        let len = self
            .quantum_chunk_width()
            .saturating_mul(dim.get().quantum)
            .min(u32::MAX / 2);
        let max = u32::MAX / 4 + 2;
        // this really shouldn't ever be larger than MAX / 8
        debug_assert!(
            len <= max,
            "chunk width is much larger than expected: {len} vs {max}",
        );
        len
    }

    /// The absolute length of the entire arq.
    pub fn absolute_length(&self, dim: impl SpaceDim) -> u64 {
        let len = (self.absolute_chunk_width(dim) as u64 * (*self.count as u64)).min(U32_LEN);
        debug_assert_eq!(
            len,
            self.to_dht_arc_range(dim).length(),
            "lengths don't match {:?}",
            self
        );
        len
    }

    /// Convert to [`DhtArcRange`] using standard topology
    pub fn to_dht_arc_range_std(&self) -> DhtArcRange {
        self.to_dht_arc_range(SpaceDimension::standard())
    }

    /// Convert to [`DhtArcRange`]
    pub fn to_dht_arc_range(&self, dim: impl SpaceDim) -> DhtArcRange {
        if is_full(dim, self.power, *self.count) {
            DhtArcRange::Full
        } else if *self.count == 0 {
            DhtArcRange::Empty
        } else {
            let (a, b) = self.to_edge_locs(dim);
            DhtArcRange::from_bounds(a, b)
        }
    }

    /// Determine the edges of this Arq in absolute coordinates ([`Loc`])
    pub fn to_edge_locs(&self, dim: impl SpaceDim) -> (Loc, Loc) {
        let start = self.start.to_offset(dim, self.power);
        let left = start.to_loc(dim, self.power);
        let right = (start + self.count).to_loc(dim, self.power) - Loc::from(1);
        (left, right)
    }

    /// Accessor
    pub fn power(&self) -> u8 {
        self.power
    }

    /// Accessor
    pub fn count(&self) -> u32 {
        self.count.into()
    }

    /// What portion of the whole circle does this arq cover?
    pub fn coverage(&self, dim: impl SpaceDim) -> f64 {
        self.absolute_length(dim) as f64 / 2f64.powf(32.0)
    }

    /// Requantize to a different power. If requantizing to a higher power,
    /// only requantize if there is no information loss due to rounding.
    /// Otherwise, return None.
    pub fn requantize(&self, new_power: u8) -> Option<Self> {
        let old_power = self.power;
        let old_count = self.count;
        if old_power < new_power {
            let factor = 2u32.pow((new_power - old_power) as u32);
            self.start.requantize_up(factor).and_then(|start| {
                let new_count = old_count / factor;
                if old_count == new_count * factor {
                    Some((start, new_power, new_count))
                } else {
                    None
                }
            })
        } else {
            let factor = 2u32.pow((old_power - new_power) as u32);
            let new_count = old_count * factor;
            Some((self.start.requantize_down(factor), new_power, new_count))
        }
        .map(|(start, power, count)| Self {
            start,
            power,
            count,
        })
    }

    /// Construct a full arq at the given power.
    /// The `count` is calculated accordingly.
    pub fn new_full(dim: impl SpaceDim, start: S, power: u8) -> Self {
        let count = pow2(32u8.saturating_sub(power + dim.get().quantum_power));
        assert!(is_full(dim, power, count));
        Self {
            start,
            power,
            count: count.into(),
        }
    }

    /// Construct a full arq at the maximum power.
    pub fn new_full_max(dim: impl SpaceDim, strat: &ArqStrat, start: S) -> Self {
        Self::new_full(dim, start, dim.get().max_power(strat))
    }

    /// This arq has full coverage
    pub fn is_full(&self, dim: impl SpaceDim) -> bool {
        is_full(dim, self.power(), self.count())
    }

    /// This arq has zero coverage
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }
}

impl Arq<Loc> {
    /// Construct an empty arq (count = 0) at the minimum power.
    pub fn new_empty(dim: impl SpaceDim, start: Loc) -> Self {
        Self {
            start,
            power: dim.get().min_power(),
            count: 0.into(),
        }
    }

    /// Reduce the power by 1
    pub fn downshift(&self) -> Self {
        Self {
            start: self.start,
            power: self.power - 1,
            count: self.count * 2,
        }
    }

    /// Increase the power by 1. If this results in rounding, return None,
    /// unless `force` is true, in which case always return Some.
    pub fn upshift(&self, force: bool) -> Option<Self> {
        let count = if force && *self.count % 2 == 1 {
            self.count + SpaceOffset(1)
        } else {
            self.count
        };
        (*count % 2 == 0).then(|| Self {
            start: self.start,
            power: self.power + 1,
            count: count / 2,
        })
    }

    /// Convert to the [`ArqBounds`] representation, which forgets about the
    /// [`Loc`] associated with this arq. Uses standard topology.
    pub fn to_bounds_std(&self) -> ArqBounds {
        self.to_bounds(SpaceDimension::standard())
    }
    /// Convert to the [`ArqBounds`] representation, which forgets about the
    /// [`Loc`] associated with this arq.
    pub fn to_bounds(&self, dim: impl SpaceDim) -> ArqBounds {
        ArqBounds {
            start: SpaceOffset::from(self.start.as_u32() / self.absolute_chunk_width(dim)),
            power: self.power,
            count: self.count,
        }
    }

    /// Get a reference to the arq's left edge in absolute coordinates.
    pub fn start_loc(&self) -> Loc {
        self.start
    }

    /// Get a mutable reference to the arq's count.
    pub fn count_mut(&mut self) -> &mut u32 {
        &mut self.count
    }

    /// Convert to [`DhtArc`]
    pub fn to_dht_arc(&self, dim: impl SpaceDim) -> DhtArc {
        let len = self.absolute_length(dim);
        DhtArc::from_start_and_len(self.start, len)
    }

    /// Convert to [`DhtArc`] using the standard SpaceDimension
    pub fn to_dht_arc_std(&self) -> DhtArc {
        self.to_dht_arc(SpaceDimension::standard())
    }

    /// Computes the Arq which most closely matches the given [`DhtArc`]
    pub fn from_dht_arc_approximate(
        dim: impl SpaceDim,
        strat: &ArqStrat,
        dht_arc: &DhtArc,
    ) -> Self {
        approximate_arq(dim, strat, dht_arc.start_loc(), dht_arc.length())
    }

    /// The two arqs represent the same interval despite having potentially different terms
    pub fn equivalent(dim: impl SpaceDim, a: &Self, b: &Self) -> bool {
        let qa = a.absolute_chunk_width(dim);
        let qb = b.absolute_chunk_width(dim);
        a.start == b.start && (a.count.wrapping_mul(qa) == b.count.wrapping_mul(qb))
    }

    /// Computes the Arq which most closely matches the given params
    pub fn from_start_and_half_len_approximate(
        dim: impl SpaceDim,
        strat: &ArqStrat,
        start: Loc,
        half_len: u32,
    ) -> Self {
        let arc = DhtArc::from_start_and_half_len(start, half_len);
        Self::from_dht_arc_approximate(dim, strat, &arc)
    }
}

impl From<&ArqBounds> for ArqBounds {
    fn from(a: &ArqBounds) -> Self {
        *a
    }
}

impl ArqBounds {
    /// The two arqs represent the same interval despite having potentially different terms
    pub fn equivalent(dim: impl SpaceDim, a: &Self, b: &Self) -> bool {
        let qa = a.absolute_chunk_width(dim);
        let qb = b.absolute_chunk_width(dim);
        *a.count == 0 && *b.count == 0
            || (a.start.wrapping_mul(qa) == b.start.wrapping_mul(qb)
                && a.count.wrapping_mul(qa) == b.count.wrapping_mul(qb))
    }

    /// Return the ArqBounds which most closely matches the given [`DhtArcRange`]
    pub fn from_interval_rounded(
        dim: impl SpaceDim,
        power: u8,
        interval: DhtArcRange,
    ) -> (Self, bool) {
        Self::from_interval_inner(dim, power, interval, true).unwrap()
    }

    /// Return the ArqBounds which is equivalent to the given [`DhtArcRange`] if it exists.
    pub fn from_interval(dim: impl SpaceDim, power: u8, interval: DhtArcRange) -> Option<Self> {
        Self::from_interval_inner(dim, power, interval, false).map(|(a, _)| a)
    }

    /// Upcast this ArqBounds to an Arq that has knowledge of its [`Loc`]
    #[cfg(any(test, feature = "test_utils"))]
    pub fn to_arq<F: FnOnce(Loc) -> Loc>(&self, dim: impl SpaceDim, f: F) -> Arq {
        Arq {
            start: f(self.start.to_loc(dim, self.power)),
            power: self.power,
            count: self.count,
        }
    }

    /// An arbitrary zero-coverage arq.
    pub fn empty(dim: impl SpaceDim, power: u8) -> Self {
        Self::from_interval(dim, power, DhtArcRange::Empty).unwrap()
    }

    fn from_interval_inner(
        dim: impl SpaceDim,
        power: u8,
        interval: DhtArcRange,
        always_round: bool,
    ) -> Option<(Self, bool)> {
        let dim = dim.get();
        match interval {
            DhtArcRange::Empty => Some((
                Self {
                    start: 0.into(),
                    power,
                    count: 0.into(),
                },
                false,
            )),
            DhtArcRange::Full => {
                assert!(power > 0);
                let full_count = 2u32.pow(32 - power as u32 - dim.quantum_power as u32);
                Some((
                    Self {
                        start: 0.into(),
                        power,
                        count: full_count.into(),
                    },
                    false,
                ))
            }
            DhtArcRange::Bounded(lo, hi) => {
                let lo = lo.as_u32();
                let hi = hi.as_u32();
                let q = dim.quantum;
                let s = 2u32.pow(power as u32) * q;
                let offset = lo / s;
                let len = if lo <= hi {
                    hi - lo + 1
                } else {
                    (2u64.pow(32) - (lo as u64) + (hi as u64) + 1) as u32
                };
                let count = len / s;
                // XXX: this is kinda wrong. The right bound of the interval
                // should be 1 less, but we'll accept if it bleeds over by 1 too.
                let rem = len % s;
                let diff = rem.min(s - rem);
                let lossless = lo == offset * s && (diff <= 1);
                if always_round || lossless {
                    Some((
                        Self {
                            start: offset.into(),
                            power,
                            count: count.into(),
                        },
                        !lossless,
                    ))
                } else {
                    tracing::warn!("{} =?= {} == {} * {}", lo, offset * s, offset, s);
                    tracing::warn!("{} =?= {} == {} * {}", len, count * s, count, s);
                    None
                }
            }
        }
    }

    /// Iterate over each segment (chunk) in the Arq
    pub fn segments(&self) -> impl Iterator<Item = SpaceSegment> + '_ {
        (0..*self.count).map(|c| SpaceSegment::new(self.power, c.wrapping_add(*self.start)))
    }

    /// Get a reference to the arq bounds's offset.
    pub fn offset(&self) -> SpaceOffset {
        self.start
    }
}

/// Just the size of a quantized arc, without a start location
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArqSize {
    /// The power
    pub power: u8,
    /// The count
    pub count: SpaceOffset,
}

impl ArqSize {
    /// Data for an empty arc
    pub fn empty() -> Self {
        Self {
            count: 0.into(),
            power: 0,
        }
    }

    /// Convert to Arq
    pub fn to_arq(&self, start: Loc) -> Arq {
        Arq::new(self.power, start, self.count)
    }

    /// Construct approximate quantized info from an arc half-length
    #[cfg(feature = "test_utils")]
    pub fn from_half_len(half_len: u32) -> Self {
        Arq::from_start_and_half_len_approximate(
            SpaceDimension::standard(),
            &ArqStrat::default(),
            0.into(),
            half_len,
        )
        .into()
    }
}

impl From<Arq> for ArqSize {
    fn from(arq: Arq) -> Self {
        Self {
            count: arq.count,
            power: arq.power,
        }
    }
}

/// Calculate whether a given combination of power and count corresponds to
/// full DHT coverage.
///
/// e.g. if the space quantum is 2^12, and the power is 14,
/// then the max power is (32 - 12) = 24. Any power 24 or greater implies fullness,
/// since even a count of 1 would be greater than 2^32.
/// Any power lower than 24 will result in full coverage with
/// count >= 2^(32 - 12 - 14) = 2^6 = 64, since it would take 64 chunks of
/// size 2^(12 + 14) to cover the full space.
pub fn is_full(dim: impl SpaceDim, power: u8, count: u32) -> bool {
    let max = 32u8.saturating_sub(dim.get().quantum_power);
    if power == 0 {
        false
    } else if power >= 32 {
        true
    } else {
        count >= pow2(max.saturating_sub(power))
    }
}

/// Calculate the unique pairing of power and count implied by a given length
/// and max number of chunks. Gives the nearest value that satisfies the constraints,
/// but may not be exact.
pub fn power_and_count_from_length(dim: impl SpaceDim, len: u64, max_chunks: u32) -> ArqSize {
    let dim = dim.get();
    assert!(len <= U32_LEN);
    let mut power = 0;
    let mut count = (len / dim.quantum as u64) as f64;
    let max = max_chunks as f64;

    while count.round() > max {
        power += 1;
        count /= 2.0;
    }
    let count = count.round() as u32;
    ArqSize {
        power,
        count: count.into(),
    }
}

/// Calculate the highest power and lowest count such that the given length is
/// represented exactly. If the length is not representable even at the quantum
/// level (power==0), return None.
pub fn power_and_count_from_length_exact(
    dim: impl SpaceDim,
    len: u64,
    min_chunks: u32,
) -> Option<ArqSize> {
    let dim = dim.get();
    assert!(len <= U32_LEN);

    let z = len.trailing_zeros();

    if z < dim.quantum_power.into() {
        return None;
    }
    let mut power = z as u8 - dim.quantum_power;
    let mut count = len >> z;

    while (count as u32) < min_chunks {
        count *= 2;
        power -= 1;
    }
    Some(ArqSize {
        power,
        count: (count as u32).into(),
    })
}

/// Given a center and a length, give Arq which matches most closely given the provided strategy
pub fn approximate_arq(dim: impl SpaceDim, strat: &ArqStrat, start: Loc, len: u64) -> Arq {
    let dim = dim.get();
    if len == 0 {
        Arq::new(dim.min_power(), start, 0.into())
    } else {
        let ArqSize { power, count } = power_and_count_from_length(dim, len, strat.max_chunks());
        let count = count.0;

        let min = strat.min_chunks() as f64;
        let max = strat.max_chunks() as f64;

        debug_assert!(
            power == 0 || count >= min as u32,
            "count < min: {} < {}",
            count,
            min
        );
        debug_assert!(
            power == 0 || count <= max as u32,
            "count > max: {} > {}",
            count,
            max
        );
        debug_assert!(count == 0 || count - 1 <= u32::MAX / dim.quantum);
        debug_assert!(power <= dim.max_power(strat), "power too large: {}", power);
        Arq::new(power, start, count.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use test_case::test_case;

    #[test]
    fn test_is_full() {
        {
            let topo = Topology::unit_zero();
            assert!(!is_full(&topo, 31, 1));
            assert!(is_full(&topo, 31, 2));
            assert!(is_full(&topo, 31, 3));

            assert!(!is_full(&topo, 30, 3));
            assert!(is_full(&topo, 30, 4));
            assert!(is_full(&topo, 29, 8));

            assert!(is_full(&topo, 1, 2u32.pow(31)));
            assert!(!is_full(&topo, 1, 2u32.pow(31) - 1));
            assert!(is_full(&topo, 2, 2u32.pow(30)));
            assert!(!is_full(&topo, 2, 2u32.pow(30) - 1));
        }
        {
            let topo = Topology::standard_epoch_full();
            assert!(!is_full(&topo, 31 - 12, 1));
            assert!(is_full(&topo, 31 - 12, 2));

            // power too high, doesn't panic
            assert!(is_full(&topo, 31, 2));
            // power too low, doesn't panic
            assert!(!is_full(&topo, 1, 2));
        }
    }

    #[test]
    fn test_full_intervals() {
        let topo = Topology::unit_zero();
        let full1 = Arq::<Loc>::new_full(&topo, 0u32.into(), 29);
        let full2 = Arq::<Loc>::new_full(&topo, 2u32.pow(31).into(), 25);
        assert!(matches!(full1.to_dht_arc_range(&topo), DhtArcRange::Full));
        assert!(matches!(full2.to_dht_arc_range(&topo), DhtArcRange::Full));
    }

    #[test]
    fn arq_requantize() {
        let c = Arq {
            start: Loc::from(42u32),
            power: 20,
            count: SpaceOffset(10),
        };

        let rq = |c: &Arq, p| (*c).requantize(p);

        assert_eq!(rq(&c, 18).map(|c| *c.count), Some(40));
        assert_eq!(rq(&c, 19).map(|c| *c.count), Some(20));
        assert_eq!(rq(&c, 20).map(|c| *c.count), Some(10));
        assert_eq!(rq(&c, 21).map(|c| *c.count), Some(5));
        assert_eq!(rq(&c, 22).map(|c| *c.count), None);
        assert_eq!(rq(&c, 23).map(|c| *c.count), None);
        assert_eq!(rq(&c, 24).map(|c| *c.count), None);

        let c = Arq {
            start: Loc::from(42u32),
            power: 20,
            count: SpaceOffset(256),
        };

        assert_eq!(rq(&c, 12).map(|c| *c.count), Some(256 * 256));
        assert_eq!(rq(&c, 28).map(|c| *c.count), Some(1));
        assert_eq!(rq(&c, 29).map(|c| *c.count), None);
    }

    #[test]
    fn test_to_bounds() {
        let topo = Topology::unit_zero();
        let pow: u8 = 4;
        {
            let a = Arq::new(pow, (2u32.pow(pow.into()) - 1).into(), 16.into());
            let b = a.to_bounds(&topo);
            assert_eq!(b.offset(), SpaceOffset(0));
            assert_eq!(b.count(), 16);
        }
        {
            let a = Arq::new(pow, 4u32.into(), 18.into());
            let b = a.to_bounds(&topo);
            assert_eq!(b.count(), 18);
        }
    }

    #[test]
    fn from_interval_regression() {
        let topo = Topology::unit_zero();
        let i = DhtArcRange::Bounded(4294967040u32.into(), 511.into());
        assert!(ArqBounds::from_interval(&topo, 8, i).is_some());
    }

    #[test_case(2u64.pow(30), (14, 16))]
    #[test_case(2u64.pow(31), (15, 16))]
    #[test_case(2u64.pow(32), (16, 16))]
    #[test_case(128 * 2u64.pow(24), (15, 16))]
    #[test_case((128 + 16) * 2u64.pow(24), (16, 9))]
    fn test_power_and_count_from_length(len: u64, expected: (u8, u32)) {
        let topo = Topology::standard_epoch_full();
        let ArqSize { power, count } = power_and_count_from_length(&topo, len, 16);
        assert_eq!((power, count.0), expected);
        assert_eq!(
            2u64.pow(power as u32 + topo.space.quantum_power as u32) * count.0 as u64,
            len
        );
    }

    #[test_case(2u64.pow(30), (15, 8))]
    #[test_case(2u64.pow(31), (16, 8))]
    #[test_case(2u64.pow(32), (17, 8))]
    #[test_case((128 + 16 + 8) * 2u64.pow(24), (15, 19))]
    #[test_case((128 + 16 + 8 + 4 + 2) * 2u64.pow(24), (13, 79))]
    fn test_power_and_count_from_length_exact(len: u64, expected: (u8, u32)) {
        let topo = Topology::standard_epoch_full();
        let ArqSize { power, count } = power_and_count_from_length_exact(&topo, len, 8).unwrap();
        assert_eq!((power, count.0), expected);
        assert_eq!(
            2u64.pow(power as u32 + topo.space.quantum_power as u32) * count.0 as u64,
            len
        );
    }

    proptest::proptest! {

        #[test]
        fn test_to_edge_locs(power in 0u8..16, count in 8u32..16, loc: u32) {
            // We use powers from 0 to 16 because with standard space topology,
            // the quantum size is 2^12, and the max count is 16 which is 2^4,
            // so any power greater than 16 could result in an overflow.
            let topo = Topology::standard_epoch_full();
            let a = Arq::new(power, Loc::from(loc), SpaceOffset(count));
            let (left, right) = a.to_edge_locs(&topo);
            let p = pow2(power);
            assert_eq!(left.as_u32() % p, 0);
            assert_eq!(right.as_u32().wrapping_add(1) % p, 0);

            assert_eq!(a.absolute_length(&topo), (right - left).as_u32() as u64 + 1);
        }

        #[test]
        fn test_preserve_ordering_for_bounds(mut centers: Vec<u32>, count in 0u32..8, power in 0u8..16) {
            let topo = Topology::standard_epoch_full();

            // given a list of sorted centerpoints
            centers.sort();

            // build identical arqs at each centerpoint and convert them to ArqBounds
            let arqs: Vec<_> = centers.into_iter().map(|c| Arq::new(power, c.into(), count.into())).collect();
            let mut bounds: Vec<_> = arqs.into_iter().map(|a| a.to_bounds(&topo)).enumerate().collect();

            // Ensure the list of ArqBounds also grows monotonically.
            // However, there may be one point at which monotonicity is broken,
            // corresponding to the left edge wrapping around.
            bounds.sort_by_key(|(_, b)| b.to_edge_locs(&topo).0);

            let mut prev = 0;
            let mut split = None;
            for (i, (ix, _)) in bounds.iter().enumerate() {
                if prev > *ix {
                    split = Some(i);
                    break;
                }
                prev = *ix;
            }

            // Split the list of bounds in two, if a discontinuity was found,
            // and check the monotonicity of each piece separately.
            let (b1, b2) = bounds.split_at(split.unwrap_or(0));
            let ix1: Vec<_> = b1.iter().map(|(i, _)| i).collect();
            let ix2: Vec<_> = b2.iter().map(|(i, _)| i).collect();
            let mut ix1s = ix1.clone();
            let mut ix2s = ix2.clone();
            ix1s.sort();
            ix2s.sort();
            assert_eq!(ix1, ix1s);
            assert_eq!(ix2, ix2s);
        }

        #[test]
        fn dht_arc_roundtrip_unit_topo(center: u32, pow in 4..29u8, count in 0..8u32) {
            let topo = Topology::unit_zero();
            let length = count as u64 * 2u64.pow(pow as u32) / 2 * 2;
            let strat = ArqStrat::default();
            let arq = approximate_arq(&topo, &strat, center.into(), length);
            let arc = arq.to_dht_arc(&topo);
            assert_eq!(arq.absolute_length(&topo), arc.length());
            let arq2 = Arq::from_dht_arc_approximate(&topo, &strat, &arc);
            assert_eq!(arq, arq2);
            let arc2 = arq2.to_dht_arc(&topo);
            assert_eq!(arc.range(), arc2.range());
        }

        #[test]
        fn dht_arc_roundtrip_standard_topo(center: u32, pow in 0..16u8, count in 0..16u32) {
            let topo = Topology::standard_epoch_full();
            let length = count as u64 * 2u64.pow(pow as u32) / 2 * 2;
            let strat = ArqStrat::default();
            let arq = approximate_arq(&topo, &strat, center.into(), length);
            let arc = arq.to_dht_arc(&topo);
            assert_eq!(arq.absolute_length(&topo), arc.length());
            let arq2 = Arq::from_dht_arc_approximate(&topo, &strat, &arc);
            assert!(Arq::<Loc>::equivalent(&topo, &arq, &arq2));
            let arc2 = arq2.to_dht_arc(&topo);
            assert_eq!(arc.range(), arc2.range());
        }

        #[test]
        fn arc_interval_roundtrip(center: u32, pow in 0..16u8, count in 0..8u32) {
            let topo = Topology::standard_epoch_full();
            let length = count as u64 * 2u64.pow(pow as u32) / 2 * 2;
            let strat = ArqStrat::default();
            let arq = approximate_arq(&topo, &strat, center.into(), length).to_bounds(&topo);
            let interval = arq.to_dht_arc_range(&topo);
            let arq2 = ArqBounds::from_interval(&topo, arq.power(), interval).unwrap();
            assert!(ArqBounds::equivalent(&topo, &arq, &arq2));
        }
    }
}
