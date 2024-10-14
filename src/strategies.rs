//! Provides the strategies used in stubborn io items
use alloc::boxed::Box;
use core::time::Duration;
use dyn_clone::DynClone;
use rand::{Rng, RngCore, SeedableRng};

#[cfg(feature = "std")]
use rand::rngs::StdRng;

#[cfg(not(feature = "std"))]
use rand::rngs::SmallRng;

pub trait RngCoreClone: RngCore + Send + Sync + DynClone {}
dyn_clone::clone_trait_object!(RngCoreClone);

#[derive(Clone)]
struct RngCoreCompat<R>(R);
impl<R> RngCore for RngCoreCompat<R>
where
    R: RngCore,
{
    fn next_u32(&mut self) -> u32 {
        self.0.next_u32()
    }

    fn next_u64(&mut self) -> u64 {
        self.0.next_u64()
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.0.fill_bytes(dest)
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand::Error> {
        self.0.try_fill_bytes(dest)
    }
}
impl<R> RngCoreClone for RngCoreCompat<R> where R: RngCore + Send + Sync + Clone {}

/// Type used for defining the exponential backoff strategy.
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use stream_reconnect::{ReconnectOptions, strategies::ExpBackoffStrategy};
///
/// // With the below strategy, the stubborn-io item will try to reconnect infinitely,
/// // waiting an exponentially increasing (by 2) value with 5% random jitter. Once the
/// // wait would otherwise exceed the maxiumum of 30 seconds, it will instead wait 30
/// // seconds.
///
/// let options = ReconnectOptions::new().with_retries_generator(|| {
///     ExpBackoffStrategy::new(Duration::from_secs(1), 2.0, 0.05)
///         .with_max(Duration::from_secs(30))
/// });
/// ```
pub struct ExpBackoffStrategy {
    min: Duration,
    max: Option<Duration>,
    factor: f64,
    jitter: f64,
    rng: Option<Box<dyn RngCoreClone + Send + Sync>>,
}

impl ExpBackoffStrategy {
    pub fn new(min: Duration, factor: f64, jitter: f64) -> Self {
        Self {
            min,
            max: None,
            factor,
            jitter,
            rng: None,
        }
    }

    /// Set the exponential backoff strategy's maximum wait value to the given duration.
    /// Otherwise, this value will be the maximum possible duration.
    pub fn with_max(mut self, max: Duration) -> Self {
        self.max = Some(max);
        self
    }

    /// Set the seed used to generate jitter. Otherwise, will set RNG via entropy.
    pub fn with_seed(self, seed: u64) -> Self {
        self.with_rng(RngCoreCompat({
            #[cfg(feature = "std")]
            {
                StdRng::seed_from_u64(seed)
            }
            #[cfg(not(feature = "std"))]
            {
                SmallRng::seed_from_u64(seed)
            }
        }))
    }

    /// Set the RNG used to generate jitter. Otherwise, will set RNG via entropy.
    pub fn with_rng<R: RngCoreClone + Send + Sync + 'static>(mut self, rng: R) -> Self {
        self.rng = Some(Box::new(rng));
        self
    }
}

impl Default for ExpBackoffStrategy {
    fn default() -> Self {
        Self {
            min: Duration::from_secs(4),
            max: Some(Duration::from_secs(30 * 60)),
            factor: 2.0,
            jitter: 0.05,
            rng: None,
        }
    }
}

impl IntoIterator for ExpBackoffStrategy {
    type Item = Duration;
    type IntoIter = ExpBackoffIter;

    fn into_iter(self) -> Self::IntoIter {
        let init = self.min.as_secs_f64();
        let rng = self.rng.clone().unwrap_or_else(|| {
            Box::new(RngCoreCompat({
                #[cfg(feature = "std")]
                {
                    StdRng::from_entropy()
                }
                #[cfg(not(feature = "std"))]
                {
                    SmallRng::from_entropy()
                }
            }))
        });
        ExpBackoffIter {
            strategy: self,
            init,
            pow: 0,
            rng,
        }
    }
}

/// Iterator class for [ExpBackoffStrategy]
pub struct ExpBackoffIter {
    strategy: ExpBackoffStrategy,
    init: f64,
    pow: u32,
    rng: Box<dyn RngCoreClone>,
}

impl Iterator for ExpBackoffIter {
    type Item = Duration;

    fn next(&mut self) -> Option<Self::Item> {
        let base = self.init * self.strategy.factor.powf(self.pow as f64);
        let jitter = base * self.strategy.jitter * (self.rng.gen::<f64>() * 2. - 1.);
        let current = Duration::from_secs_f64(base + jitter);
        self.pow += 1;
        match self.strategy.max {
            Some(max) => Some(max.min(current)),
            None => Some(current),
        }
    }
}

#[cfg(test)]
mod test {
    use super::ExpBackoffStrategy;
    use core::time::Duration;

    #[test]
    fn test_exponential_backoff_jitter_values() {
        let mut backoff_iter = ExpBackoffStrategy::new(Duration::from_secs(1), 2., 0.1)
            .with_seed(0)
            .into_iter();
        let expected_values = [
            1.046222683,
            2.109384073,
            3.620675707,
            8.134654819,
            15.238946024,
            33.740716196,
            60.399320456,
            135.51906449,
            268.766127569,
        ];
        for expected in expected_values {
            let value = backoff_iter.next().unwrap().as_secs_f64();
            assert!(
                (value - expected).abs() < 0.0001,
                "{} != {}",
                value,
                expected
            );
        }
    }

    #[test]
    fn test_exponential_backoff_max_value() {
        let mut backoff_iter = ExpBackoffStrategy::new(Duration::from_secs(1), 2., 0.0)
            .with_seed(0)
            .with_max(Duration::from_secs(8))
            .into_iter();
        let expected_values = [1.0, 2.0, 4.0, 8.0, 8.0];
        for expected in expected_values {
            let value = backoff_iter.next().unwrap().as_secs_f64();
            assert!(
                (value - expected).abs() < 0.0001,
                "{} != {}",
                value,
                expected
            );
        }
    }
}
